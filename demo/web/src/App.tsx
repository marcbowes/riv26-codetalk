import { useState } from "react";
import { type AppConfig, REGION_NAMES, type RegionConfig } from "./config";
import { RegionColumn } from "./RegionColumn";
import { BOTH, pickRegions, withRegionParam } from "./regions";

export type View = { kind: "list"; category?: string } | { kind: "post"; id: string };

const regionName = (r: RegionConfig) => r.name ?? REGION_NAMES[r.region] ?? r.region;

export function App({ config }: { config: AppConfig }) {
  const [initial] = useState(() => pickRegions(new URLSearchParams(window.location.search).get("region"), config.regions));
  const [both, setBoth] = useState(initial.both);
  // Navigation is shared: with both Regions showing, open a post in one column
  // and the other follows, so a comment written on the left can be watched
  // arriving on the right.
  const [view, setView] = useState<View>({ kind: "list" });

  const showBoth = (on: boolean) => {
    setBoth(on);
    const single = config.regions[initial.single];
    const value = on ? BOTH : initial.fromParam ? single?.region : undefined;
    window.history.replaceState(window.history.state, "", withRegionParam(window.location.href, value));
  };

  const shown = config.regions
    .map((r, i) => ({ r, i }))
    .filter(({ i }) => both || i === initial.single);
  const named = config.regions.filter((r) => r.region !== "local");
  const witness = config.witnessRegion;
  const witnessText = witness ? ` Witness ${both ? "Region " : ""}${witness} (${REGION_NAMES[witness] ?? witness}).` : "";
  const here = config.regions[initial.single];

  return (
    <div className="app">
      <header className="masthead">
        <h1 className="wordmark">DAT401 blog</h1>
        <p className="tagline">
          {named.length === 2 ? (
            both ? (
              <>
                One Aurora DSQL database, read and written in {named[0]?.region} and {named[1]?.region} at the same time.
                {witnessText}
              </>
            ) : (
              <>
                Served from {here?.region} ({here ? regionName(here) : ""}). One Aurora DSQL database, read and written in{" "}
                {named[0]?.region} and {named[1]?.region}.{witnessText}
              </>
            )
          ) : both ? (
            <>Local development: both columns talk to {config.regions[0]?.apiUrl}.</>
          ) : (
            <>Local development: talking to {here?.apiUrl}.</>
          )}
        </p>
        {config.regions.length > 1 ? (
          <label className="toggle">
            <input type="checkbox" checked={both} onChange={(e) => showBoth(e.target.checked)} />
            Show both Regions
          </label>
        ) : null}
      </header>
      <main className="columns" style={{ "--cols": shown.length } as React.CSSProperties}>
        {shown.map(({ r, i }) => (
          <RegionColumn key={i} index={i} config={r} view={view} navigate={setView} />
        ))}
      </main>
    </div>
  );
}
