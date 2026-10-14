// Word lists and templates for plausible-looking seed content. No external
// generator dependency: everything here is deterministic given the PRNG seed.
import type { Category } from "@dat401/api/categories";

export const FIRST_NAMES = [
  "Amara", "Ben", "Carmen", "Dev", "Elena", "Farah", "Gabe", "Hana", "Ivan", "Jia", "Kofi", "Lena",
  "Mateo", "Nadia", "Omar", "Priya", "Quinn", "Rosa", "Sam", "Tariq", "Uma", "Viktor", "Wen", "Ximena",
  "Yusuf", "Zoe", "Aiko", "Bruno", "Chloe", "Diego", "Esme", "Felix", "Grace", "Hugo", "Isla", "Jonas",
  "Kai", "Leila", "Marcus", "Noor", "Oscar", "Pia", "Rafael", "Sofia", "Theo", "Valeria", "Will", "Yara",
  "Anil", "Beatriz", "Cyrus", "Dana", "Emeka", "Freya", "Goran", "Helga", "Ines", "Jamal", "Keiko", "Luca",
] as const;

export const LAST_NAMES = [
  "Adeyemi", "Bauer", "Castillo", "Dubois", "Eriksen", "Fischer", "Garcia", "Haddad", "Ito", "Jensen",
  "Kowalski", "Lindqvist", "Moreau", "Nakamura", "Okafor", "Petrov", "Quintero", "Rossi", "Silva", "Tanaka",
  "Ueda", "Varga", "Walsh", "Xu", "Yilmaz", "Zhang", "Abbott", "Byrne", "Chen", "Dlamini", "Esposito",
  "Fernandes", "Gupta", "Horvat", "Iqbal", "Jovanovic", "Kim", "Lopez", "Mensah", "Novak", "O'Brien",
  "Park", "Reyes", "Schmidt", "Takahashi", "Usman", "Vasquez", "Weber", "Yamamoto", "Zielinski",
] as const;

export const TOPICS: Record<Category, readonly string[]> = {
  databases: [
    "Postgres", "Aurora DSQL", "B-tree indexes", "MVCC", "query planners", "secondary indexes",
    "foreign keys", "JSONB", "connection pooling", "schema migrations", "optimistic concurrency",
    "isolation levels", "UUID primary keys", "EXPLAIN ANALYZE", "expression indexes", "covering indexes",
  ],
  "distributed-systems": [
    "consensus", "clock synchronization", "quorums", "multi-Region writes", "idempotency keys",
    "retries with jitter", "leader election", "exactly-once delivery", "linearizability", "backpressure",
    "failure detectors", "change data capture", "strong consistency", "partition tolerance",
  ],
  serverless: [
    "AWS Lambda", "cold starts", "function URLs", "Step Functions", "EventBridge", "SQS",
    "concurrency limits", "event-driven design", "scale to zero", "Lambda extensions", "Kinesis consumers",
  ],
  performance: [
    "p99 latency", "flame graphs", "connection reuse", "N+1 queries", "caching", "load testing",
    "tail latency", "batching", "memory allocation", "async I/O", "benchmarks", "query plans",
  ],
  security: [
    "IAM roles", "least privilege", "token-based auth", "secret rotation", "TLS everywhere",
    "SQL injection", "audit logging", "KMS keys", "resource policies", "threat modeling",
  ],
  devops: [
    "AWS CDK", "blue/green deploys", "feature flags", "observability", "CloudWatch dashboards", "runbooks",
    "on-call", "CI pipelines", "infrastructure as code", "canary releases", "rollbacks",
  ],
  frontend: [
    "React", "Vite", "optimistic UI", "polling", "accessibility", "CSS grid", "design tokens",
    "bundle size", "form validation", "TypeScript", "client-side caching",
  ],
  career: [
    "code review", "design docs", "mentoring", "giving talks", "technical debt", "estimation",
    "saying no", "staff engineering", "learning in public", "interviewing", "writing well",
  ],
};

export const TAGS: Record<Category, readonly string[]> = {
  databases: ["postgres", "dsql", "sql", "indexes", "jsonb", "transactions", "occ", "schema", "explain"],
  "distributed-systems": ["consistency", "multi-region", "replication", "cdc", "retries", "idempotency", "clocks"],
  serverless: ["lambda", "scaling", "event-driven", "cold-starts", "kinesis", "sqs", "cost"],
  performance: ["latency", "profiling", "benchmarks", "caching", "throughput", "p99"],
  security: ["iam", "auth", "encryption", "least-privilege", "compliance", "secrets"],
  devops: ["cdk", "ci-cd", "monitoring", "deployments", "incidents", "observability"],
  frontend: ["react", "typescript", "css", "ux", "accessibility", "vite"],
  career: ["growth", "leadership", "writing", "teams", "mentoring", "speaking"],
};

export const TITLE_TEMPLATES = [
  "What I learned running {topic} in production",
  "{Topic}, explained with a whiteboard",
  "A practical guide to {topic}",
  "Why {topic} is harder than it looks",
  "{n} mistakes I made with {topic}",
  "Stop worrying about {topic}",
  "How we cut p99 in half with {topic}",
  "The case for {topic}",
  "{Topic} from first principles",
  "Debugging {topic}: a field guide",
  "Notes on {topic}",
  "{Topic} without the hype",
  "When {topic} goes wrong",
  "Rethinking {topic} for multi-Region apps",
  "A gentle introduction to {topic}",
  "{Topic} vs {topic2}: what actually matters",
  "Lessons from a year of {topic}",
  "The hidden cost of {topic}",
  "{Topic} in {n} minutes",
  "Everything I wish I knew about {topic}",
] as const;

export const HEADINGS = [
  "Background", "The problem", "What we tried first", "What actually worked", "Trade-offs",
  "The numbers", "What surprised us", "Takeaways", "If you only read one section", "What's next",
] as const;

export const SENTENCES = [
  "The first time I ran into {topic}, I assumed it would be a quick fix.",
  "It turns out that {topic} interacts with {topic2} in ways the docs don't spell out.",
  "Most teams don't notice until traffic grows by an order of magnitude.",
  "We measured everything before changing anything, which saved us from at least two bad ideas.",
  "The fix was smaller than the investigation: about forty lines of code and one config change.",
  "If you only remember one thing from this post, make it this: measure first.",
  "Here is the mental model that finally made {topic} click for me.",
  "There's a trade-off here, and pretending otherwise is how outages start.",
  "In our case the bottleneck wasn't {topic} at all; it was {topic2}.",
  "A small experiment is worth more than a long design review.",
  "{Topic} rewards boring, predictable designs.",
  "The numbers surprised us: p50 barely moved, but p99 dropped by more than half.",
  "We wrote a runbook afterwards, and it has already paid for itself.",
  "None of this is new, but it's easy to forget under deadline pressure.",
  "Retries are only safe when the operation is idempotent, so we made sure it was.",
  "We started with the simplest thing that could possibly work.",
  "The documentation covers the happy path well; this post is about the other paths.",
  "Our dashboards told a different story than our intuition did.",
  "That one change removed an entire class of pages for whoever was on call.",
  "It's worth asking what happens when this runs in two Regions at once.",
  "We kept a list of every assumption and checked them one by one.",
  "Load testing with realistic data was the turning point.",
  "{Topic} is a tool, not a goal.",
  "The best part is that the code got simpler, not more complicated.",
  "Afterwards, we wrote down what we'd do differently next time.",
  "A teammate asked a simple question that unravelled the whole thing.",
  "We could have kept scaling up, but that would only have moved the problem.",
  "The real lesson was about {topic2}, even though we set out to fix {topic}.",
  "Nothing about this required heroics, just patience and good telemetry.",
  "It took three tries to get the abstraction right, and the third one was the smallest.",
  "Once we stopped fighting {topic}, the rest of the design fell into place.",
  "Every shortcut we took early on showed up again later as an incident.",
  "The surprising part was how little configuration it needed.",
  "We added one metric, and it answered questions we'd been arguing about for weeks.",
  "The worst bugs hide in the retry path, so that's where we started reading.",
  "You don't need to get this perfect on day one; you need to be able to change it.",
] as const;

export const COMMENTS = [
  "Great write-up, thanks for sharing the numbers.",
  "We hit exactly this last quarter. Wish I'd read this first.",
  "How did you test this before rolling it out?",
  "Does this still hold once you add {topic2}?",
  "+1 to measuring first.",
  "The section on trade-offs is the best part.",
  "Interesting. We went the other way and it worked for us, but our traffic is much lower.",
  "Bookmarking this for our next design review.",
  "Any chance you could share the dashboard you used?",
  "This matches what we saw in production.",
  "I'd love a follow-up on the failure cases.",
  "Clear and practical. More posts like this please.",
  "What would you change if you had to do it again?",
  "We tried something similar with {topic}; the retry logic was the tricky part.",
  "Sending this to my team.",
  "How long did the migration take end to end?",
  "The bit about idempotency deserves its own post.",
  "Did you consider {topic2} instead?",
  "Really helpful, especially the part about what didn't work.",
  "I disagree a little on the conclusion, but the analysis is solid.",
  "This is the clearest explanation of {topic} I've read.",
  "Thanks! Printed this out for our reading group.",
  "Curious how this behaves under a cold start.",
  "Ha, the runbook line hit close to home.",
] as const;
