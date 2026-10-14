// Timestamps stay strings end to end so DSQL's microseconds survive: a JS Date
// would truncate to milliseconds, and a `?before=` cursor built from a
// truncated value can skip rows that share the millisecond.

const PG_TS = /^(\d{4})-(\d{2})-(\d{2})[ T](\d{2}):(\d{2}):(\d{2})(\.\d+)?(Z|[+-]\d{2}(?::?\d{2})?)?$/;

/**
 * Postgres timestamptz text ("2026-10-14 12:00:00.123456+00", any session
 * TimeZone) → ISO-8601 in UTC with the original fractional digits
 * ("2026-10-14T12:00:00.123456Z").
 */
export function pgTimestampToIso(value: string): string {
  const m = PG_TS.exec(value.trim());
  if (!m) {
    const ms = Date.parse(value);
    if (Number.isNaN(ms)) throw new Error(`unrecognized timestamp: ${value}`);
    return new Date(ms).toISOString();
  }
  const [, y, mo, d, h, mi, s, frac = "", off = "Z"] = m;
  let offsetMinutes = 0;
  if (off !== "Z") {
    const sign = off.startsWith("-") ? -1 : 1;
    const digits = off.slice(1).replace(":", "");
    offsetMinutes = sign * (Number(digits.slice(0, 2)) * 60 + Number(digits.slice(2, 4) || "0"));
  }
  // Offsets are whole minutes, so only the integer seconds need shifting.
  const utc = Date.UTC(Number(y), Number(mo) - 1, Number(d), Number(h), Number(mi), Number(s)) - offsetMinutes * 60_000;
  return `${new Date(utc).toISOString().slice(0, 19)}${frac}Z`;
}
