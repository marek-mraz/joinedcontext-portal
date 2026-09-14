import type { Row } from "../ngsi";
import { aggregate, format } from "../ngsi";
import type { StatItem } from "../spec";

/** A row of tiles: one number each, over the rows the filters left. */
export function Stats({ rows, items }: { rows: Row[]; items: StatItem[] }) {
  return (
    <div className="stats">
      {items.map((item) => {
        const value = aggregate(rows, item.agg, item.attr);
        return (
          <div className="stat" key={`${item.label}-${item.agg}-${item.attr ?? ""}`}>
            <span className="stat-value">
              {value === null ? "–" : format(value)}
              {item.unit && value !== null ? <span className="stat-unit"> {item.unit}</span> : null}
            </span>
            <span className="stat-label">{item.label}</span>
          </div>
        );
      })}
    </div>
  );
}
