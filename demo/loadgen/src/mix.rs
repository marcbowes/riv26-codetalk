//! Traffic mix: which API call to make next.

/// One kind of API call.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Op {
    /// `GET /posts?limit=N`
    ListPosts,
    /// `GET /posts?limit=N&category=<random>`
    ListByCategory,
    /// `GET /posts/:id`
    GetPost,
    /// `POST /posts/:id/comments`
    CreateComment,
    /// `POST /posts`
    CreatePost,
}

pub const NOPS: usize = 5;

impl Op {
    pub const ALL: [Op; NOPS] = [
        Op::ListPosts,
        Op::ListByCategory,
        Op::GetPost,
        Op::CreateComment,
        Op::CreatePost,
    ];

    pub fn index(self) -> usize {
        self as usize
    }

    pub fn label(self) -> &'static str {
        match self {
            Op::ListPosts => "GET /posts",
            Op::ListByCategory => "GET /posts?category=",
            Op::GetPost => "GET /posts/:id",
            Op::CreateComment => "POST /posts/:id/comments",
            Op::CreatePost => "POST /posts",
        }
    }

    /// Stable key for the JSON report.
    pub fn key(self) -> &'static str {
        match self {
            Op::ListPosts => "listPosts",
            Op::ListByCategory => "listPostsByCategory",
            Op::GetPost => "getPost",
            Op::CreateComment => "createComment",
            Op::CreatePost => "createPost",
        }
    }

    pub fn is_write(self) -> bool {
        matches!(self, Op::CreateComment | Op::CreatePost)
    }
}

/// `--mix READ/WRITE` in percent.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ReadWrite {
    pub read: f64,
    pub write: f64,
}

fn parse_pct(s: &str) -> Result<f64, String> {
    let v: f64 = s
        .trim()
        .trim_end_matches('%')
        .parse()
        .map_err(|_| format!("{s:?} is not a number"))?;
    if !(0.0..=100.0).contains(&v) {
        return Err(format!("{v} is not between 0 and 100"));
    }
    Ok(v)
}

/// Accepts `90/10`, `90:10` or just `90` (the rest are writes).
pub fn parse_read_write(s: &str) -> Result<ReadWrite, String> {
    let parts: Vec<&str> = s.split(['/', ':']).collect();
    let (read, write) = match parts.as_slice() {
        [r] => {
            let r = parse_pct(r)?;
            (r, 100.0 - r)
        }
        [r, w] => (parse_pct(r)?, parse_pct(w)?),
        _ => return Err(format!("expected READ/WRITE like 90/10, got {s:?}")),
    };
    if (read + write - 100.0).abs() > 0.01 {
        return Err(format!(
            "read and write must add up to 100 (got {read} + {write})"
        ));
    }
    Ok(ReadWrite { read, write })
}

/// Mix percentages from the command line.
#[derive(Clone, Copy, Debug)]
pub struct MixConfig {
    /// Percent of all requests that are reads.
    pub read_pct: f64,
    /// Percent of reads that list posts (the rest fetch a single post).
    pub list_pct: f64,
    /// Percent of list reads that filter by a category.
    pub category_pct: f64,
    /// Percent of writes that create a post (the rest add a comment).
    pub new_post_pct: f64,
}

/// Rolls are drawn uniformly from `0..PPM` (parts per million).
pub const PPM: u32 = 1_000_000;

#[derive(Clone, Debug)]
pub struct Mix {
    cumulative: [u32; NOPS],
    shares: [f64; NOPS],
}

impl Mix {
    pub fn new(cfg: &MixConfig) -> Result<Mix, String> {
        for (name, v) in [
            ("read share", cfg.read_pct),
            ("--list-share", cfg.list_pct),
            ("--category-share", cfg.category_pct),
            ("--new-post-share", cfg.new_post_pct),
        ] {
            if !(0.0..=100.0).contains(&v) {
                return Err(format!("{name} must be between 0 and 100, got {v}"));
            }
        }
        let read = cfg.read_pct / 100.0;
        let write = 1.0 - read;
        let list = read * cfg.list_pct / 100.0;
        let cat = cfg.category_pct / 100.0;
        let new_post = cfg.new_post_pct / 100.0;
        let shares = [
            list * (1.0 - cat),
            list * cat,
            read - list,
            write * (1.0 - new_post),
            write * new_post,
        ];
        let mut cumulative = [0u32; NOPS];
        let mut sum = 0.0;
        for (i, s) in shares.iter().enumerate() {
            sum += s;
            cumulative[i] = ((sum * PPM as f64).round() as u32).min(PPM);
        }
        // Make sure the last op with a non-zero share absorbs rounding, so a
        // zero-share op can never be picked.
        let last = shares
            .iter()
            .rposition(|s| *s > 0.0)
            .ok_or("the mix has no operations")?;
        for c in cumulative.iter_mut().skip(last) {
            *c = PPM;
        }
        Ok(Mix { cumulative, shares })
    }

    /// Pick an op for a roll in `0..PPM`.
    pub fn pick(&self, roll: u32) -> Op {
        for (i, c) in self.cumulative.iter().enumerate() {
            if roll < *c {
                return Op::ALL[i];
            }
        }
        Op::ALL[NOPS - 1]
    }

    pub fn share(&self, op: Op) -> f64 {
        self.shares[op.index()]
    }

    pub fn describe(&self) -> String {
        Op::ALL
            .iter()
            .filter(|op| self.share(**op) > 0.0)
            .map(|op| format!("{} {}%", op.label(), fmt_pct(self.share(*op) * 100.0)))
            .collect::<Vec<_>>()
            .join(", ")
    }
}

fn fmt_pct(v: f64) -> String {
    if (v - v.round()).abs() < 1e-9 {
        format!("{v:.0}")
    } else {
        format!("{v:.1}")
    }
}

/// What the id pool can currently support.
#[derive(Clone, Copy, Debug)]
pub struct Available {
    pub posts: bool,
    pub users: bool,
    pub categories: bool,
}

/// Swap an op for one that can run with the ids we have (e.g. no posts yet).
pub fn fallback(op: Op, have: Available) -> Op {
    match op {
        Op::ListByCategory if !have.categories => Op::ListPosts,
        Op::GetPost if !have.posts => Op::ListPosts,
        Op::CreateComment if !have.users => Op::ListPosts,
        Op::CreateComment if !have.posts => Op::CreatePost,
        Op::CreatePost if !have.users => Op::ListPosts,
        other => other,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn default_mix() -> Mix {
        Mix::new(&MixConfig {
            read_pct: 90.0,
            list_pct: 40.0,
            category_pct: 50.0,
            new_post_pct: 5.0,
        })
        .unwrap()
    }

    #[test]
    fn default_shares() {
        let m = default_mix();
        let expect = [0.18, 0.18, 0.54, 0.095, 0.005];
        for (op, e) in Op::ALL.iter().zip(expect) {
            assert!((m.share(*op) - e).abs() < 1e-12, "{op:?}");
        }
        let total: f64 = Op::ALL.iter().map(|op| m.share(*op)).sum();
        assert!((total - 1.0).abs() < 1e-12);
    }

    #[test]
    fn pick_boundaries() {
        let m = default_mix();
        assert_eq!(m.pick(0), Op::ListPosts);
        assert_eq!(m.pick(179_999), Op::ListPosts);
        assert_eq!(m.pick(180_000), Op::ListByCategory);
        assert_eq!(m.pick(360_000), Op::GetPost);
        assert_eq!(m.pick(899_999), Op::GetPost);
        assert_eq!(m.pick(900_000), Op::CreateComment);
        assert_eq!(m.pick(995_000), Op::CreatePost);
        assert_eq!(m.pick(PPM - 1), Op::CreatePost);
    }

    #[test]
    fn seeded_draws_match_the_shares() {
        let m = default_mix();
        let mut rng = fastrand::Rng::with_seed(42);
        let n = 1_000_000;
        let mut counts = [0usize; NOPS];
        for _ in 0..n {
            counts[m.pick(rng.u32(..PPM)).index()] += 1;
        }
        for op in Op::ALL {
            let got = counts[op.index()] as f64 / n as f64;
            assert!(
                (got - m.share(op)).abs() < 0.002,
                "{op:?}: got {got}, want {}",
                m.share(op)
            );
        }
        let writes: usize = Op::ALL
            .iter()
            .filter(|op| op.is_write())
            .map(|op| counts[op.index()])
            .sum();
        assert!((writes as f64 / n as f64 - 0.10).abs() < 0.002);
    }

    #[test]
    fn all_reads_or_all_writes() {
        let reads = Mix::new(&MixConfig {
            read_pct: 100.0,
            list_pct: 40.0,
            category_pct: 50.0,
            new_post_pct: 5.0,
        })
        .unwrap();
        let writes = Mix::new(&MixConfig {
            read_pct: 0.0,
            list_pct: 40.0,
            category_pct: 50.0,
            new_post_pct: 5.0,
        })
        .unwrap();
        let mut rng = fastrand::Rng::with_seed(1);
        for _ in 0..100_000 {
            assert!(!reads.pick(rng.u32(..PPM)).is_write());
            assert!(writes.pick(rng.u32(..PPM)).is_write());
        }
        assert!(!reads.pick(PPM - 1).is_write());
        assert!(writes.pick(0).is_write());
    }

    #[test]
    fn zero_share_ops_are_never_picked() {
        let m = Mix::new(&MixConfig {
            read_pct: 90.0,
            list_pct: 100.0,
            category_pct: 0.0,
            new_post_pct: 0.0,
        })
        .unwrap();
        assert_eq!(m.pick(PPM - 1), Op::CreateComment);
        let mut rng = fastrand::Rng::with_seed(3);
        for _ in 0..200_000 {
            let op = m.pick(rng.u32(..PPM));
            assert!(matches!(op, Op::ListPosts | Op::CreateComment), "{op:?}");
        }
    }

    #[test]
    fn parses_read_write() {
        assert_eq!(
            parse_read_write("90/10"),
            Ok(ReadWrite {
                read: 90.0,
                write: 10.0
            })
        );
        assert_eq!(parse_read_write("75:25").unwrap().write, 25.0);
        assert_eq!(parse_read_write("80").unwrap().write, 20.0);
        assert_eq!(parse_read_write("100/0").unwrap().read, 100.0);
        assert_eq!(parse_read_write("99.5/0.5").unwrap().write, 0.5);
        assert!(parse_read_write("80/30").is_err());
        assert!(parse_read_write("abc").is_err());
        assert!(parse_read_write("1/2/3").is_err());
        assert!(parse_read_write("120").is_err());
    }

    #[test]
    fn falls_back_when_ids_are_missing() {
        let none = Available {
            posts: false,
            users: false,
            categories: false,
        };
        let all = Available {
            posts: true,
            users: true,
            categories: true,
        };
        for op in Op::ALL {
            assert_eq!(fallback(op, all), op);
            assert_eq!(fallback(op, none), Op::ListPosts);
        }
        let no_posts = Available {
            posts: false,
            ..all
        };
        assert_eq!(fallback(Op::CreateComment, no_posts), Op::CreatePost);
        assert_eq!(fallback(Op::GetPost, no_posts), Op::ListPosts);
        let no_cats = Available {
            categories: false,
            ..all
        };
        assert_eq!(fallback(Op::ListByCategory, no_cats), Op::ListPosts);
    }

    #[test]
    fn describe_lists_nonzero_ops() {
        let d = default_mix().describe();
        assert!(d.contains("GET /posts/:id 54%"), "{d}");
        assert!(d.contains("POST /posts 0.5%"), "{d}");
    }
}
