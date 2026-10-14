import type { Person } from "@dat401/api/types";
import { useEffect, useMemo, useRef, useState } from "react";
import { type Sample, createClient } from "./api";
import type { View } from "./App";
import { REGION_NAMES, type RegionConfig } from "./config";
import { describeRequest, median, ms } from "./format";
import { PostList } from "./PostList";
import { PostView } from "./PostView";

const HISTORY = 24;

export function RegionColumn({
  index,
  config,
  view,
  navigate,
}: {
  index: number;
  config: RegionConfig;
  view: View;
  navigate: (v: View) => void;
}) {
  const [samples, setSamples] = useState<Sample[]>([]);
  const client = useMemo(
    () => createClient(config.apiUrl, (s) => setSamples((xs) => [...xs.slice(-(HISTORY - 1)), s])),
    [config.apiUrl],
  );

  const [users, setUsers] = useState<Person[] | null>(null);
  const [categories, setCategories] = useState<string[]>([]);
  useEffect(() => {
    let live = true;
    const load = async () => {
      try {
        const [u, c] = [await client.users(200), await client.categories()];
        if (live) {
          setUsers([...u.users].sort((a, b) => a.displayName.localeCompare(b.displayName)));
          setCategories(c.categories);
        }
      } catch {
        if (live) setTimeout(load, 3_000); // the views show the error; keep trying quietly
      }
    };
    void load();
    return () => {
      live = false;
    };
  }, [client]);

  // Scroll back to the top when the view changes.
  const body = useRef<HTMLDivElement>(null);
  const viewKey = view.kind === "post" ? `post:${view.id}` : `list:${view.category ?? ""}`;
  useEffect(() => {
    body.current?.scrollTo({ top: 0 });
  }, [viewKey]);

  const reported = [...samples].reverse().find((s) => s.region)?.region;
  const region = config.region === "local" ? (reported ?? "local") : config.region;
  const name = config.name ?? REGION_NAMES[region] ?? (region === "local" ? "Your laptop" : "");

  return (
    <section className="column" style={{ "--region": `var(--region-${index % 2})` } as React.CSSProperties}>
      <RegionHeader
        region={region}
        name={name}
        samples={samples}
        mismatch={config.region !== "local" && reported && reported !== config.region ? reported : undefined}
      />
      <div className="column-body" ref={body}>
        {view.kind === "list" ? (
          <PostList
            key={viewKey}
            client={client}
            category={view.category}
            categories={categories}
            navigate={navigate}
          />
        ) : (
          <PostView
            key={viewKey}
            client={client}
            id={view.id}
            column={index}
            region={region}
            users={users}
            navigate={navigate}
          />
        )}
      </div>
    </section>
  );
}

function RegionHeader({
  region,
  name,
  samples,
  mismatch,
}: {
  region: string;
  name: string;
  samples: Sample[];
  mismatch: string | undefined;
}) {
  const last = samples.at(-1);
  const ok = samples.filter((s) => s.status > 0 && s.status < 500);
  const typical = median(ok.map((s) => s.ms));
  return (
    <header className="region-header">
      <div className="region-id">
        <h2 className="region-code">{region}</h2>
        <p className="region-name">{name}</p>
      </div>
      <dl className="readouts" aria-live="off">
        <div>
          <dt>Round trip</dt>
          <dd>{ms(last?.ms)}</dd>
        </div>
        <div>
          <dt>In Aurora DSQL</dt>
          <dd>{ms(last?.dbMs)}</dd>
        </div>
      </dl>
      <div className="history">
        <Spark samples={samples} />
        <p className="history-note">
          {last
            ? `${capitalize(describeRequest(last.method, last.path))}${last.status === 0 || last.status >= 500 ? ", failed" : ""}.${
                typical !== undefined ? ` Median ${ms(typical)} over ${ok.length}.` : ""
              }`
            : "Waiting for the first request"}
        </p>
      </div>
      {mismatch ? <p className="warning">Answered by {mismatch}, not {region}. Check config.json.</p> : null}
    </header>
  );
}

const capitalize = (t: string) => t.charAt(0).toUpperCase() + t.slice(1);

/** Recent round trips as a strip of thin bars, newest on the right. */
function Spark({ samples }: { samples: Sample[] }) {
  const W = 8;
  const H = 36;
  const max = Math.max(50, ...samples.map((s) => s.ms));
  const label = samples.length
    ? `Last ${samples.length} requests, newest ${ms(samples.at(-1)?.ms)}`
    : "No requests yet";
  return (
    <svg className="spark" width={HISTORY * W} height={H} viewBox={`0 0 ${HISTORY * W} ${H}`} role="img" aria-label={label}>
      <line x1="0" x2={HISTORY * W} y1={H - 0.5} y2={H - 0.5} className="spark-base" />
      {samples.map((s, i) => {
        const failed = s.status === 0 || s.status >= 500;
        const h = failed ? H - 2 : Math.max(3, Math.round((s.ms / max) * (H - 2)));
        const x = (HISTORY - samples.length + i) * W + 1;
        return (
          <rect
            key={`${s.at}-${i}`}
            x={x}
            y={H - h}
            width={W - 3}
            height={h}
            rx="1.5"
            className={failed ? "spark-bar failed" : "spark-bar"}
          >
            <title>
              {`${describeRequest(s.method, s.path)}: ${failed ? "failed" : ms(s.ms)}${s.dbMs !== undefined ? ` (${ms(s.dbMs)} in the database)` : ""}`}
            </title>
          </rect>
        );
      })}
    </svg>
  );
}
