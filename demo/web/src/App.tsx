import { useCallback, useState } from "react";
import { type AppConfig, REGION_NAMES } from "./config";
import { RegionColumn } from "./RegionColumn";

export type View = { kind: "list"; category?: string } | { kind: "post"; id: string };

export function App({ config }: { config: AppConfig }) {
  // Mirrored navigation: open a post in one column and the other follows, so a
  // comment written on the left can be watched arriving on the right.
  const [mirror, setMirror] = useState(true);
  const [views, setViews] = useState<View[]>(() => config.regions.map(() => ({ kind: "list" })));

  const navigate = useCallback(
    (column: number, view: View) => setViews((vs) => vs.map((v, i) => (mirror || i === column ? view : v))),
    [mirror],
  );

  const named = config.regions.filter((r) => r.region !== "local");
  const witness = config.witnessRegion;

  return (
    <div className="app">
      <header className="masthead">
        <h1 className="wordmark">DAT401 blog</h1>
        <p className="tagline">
          {named.length === 2 ? (
            <>
              One Aurora DSQL database, read and written in {named[0]?.region} and {named[1]?.region} at the same time.
              {witness ? ` Witness Region ${witness} (${REGION_NAMES[witness] ?? witness}).` : ""}
            </>
          ) : (
            <>Local development: both columns talk to {config.regions[0]?.apiUrl}.</>
          )}
        </p>
        <label className="mirror">
          <input type="checkbox" checked={mirror} onChange={(e) => setMirror(e.target.checked)} />
          Keep both columns on the same page
        </label>
      </header>
      <main className="columns" style={{ "--cols": config.regions.length } as React.CSSProperties}>
        {config.regions.map((r, i) => (
          <RegionColumn
            key={i}
            index={i}
            config={r}
            view={views[i] ?? { kind: "list" }}
            navigate={(v) => navigate(i, v)}
          />
        ))}
      </main>
    </div>
  );
}
