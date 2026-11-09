import type { TrafficSample } from "../lib/types";

interface SparklineProps {
  samples?: TrafficSample[];
  /** Which series to draw; the connection page shows download by default. */
  series?: "down" | "up";
  height?: number;
}

/**
 * A hand-rolled SVG chart. 60 points, no library, no animation on every tick
 * (only a CSS transition on the path), because the window updates once a second.
 */
export function Sparkline({ samples = [], series = "down", height = 92 }: SparklineProps) {
  const width = 520;
  const pad = 4;
  const window = 60;
  const observed = samples
    .slice(-window)
    .map((sample) => (series === "down" ? sample.downBps : sample.upBps));
  // Pad on the left with the first reading so a fresh session draws a full-width
  // line instead of a dot that looks like a rendering bug.
  const values = observed.length === 0 ? [] : [...Array(window - observed.length).fill(observed[0]), ...observed];
  const max = Math.max(...values, 1);
  const step = values.length > 1 ? (width - pad * 2) / (values.length - 1) : 0;
  if (values.length === 0) {
    return (
      <svg className="sparkline" viewBox={`0 0 ${width} ${height}`} preserveAspectRatio="none" aria-hidden="true">
        <line x1={pad} y1={height - pad} x2={width - pad} y2={height - pad} stroke="var(--line)" strokeWidth="1" />
      </svg>
    );
  }
  const line = values
    .map((value, index) => {
      const x = pad + index * step;
      const y = height - pad - (value / max) * (height - pad * 2);
      return `${index === 0 ? "M" : "L"}${x.toFixed(1)},${y.toFixed(1)}`;
    })
    .join(" ");
  const area = `${line} L${(pad + (values.length - 1) * step).toFixed(1)},${height - pad} L${pad},${height - pad} Z`;

  return (
    <svg className="sparkline" viewBox={`0 0 ${width} ${height}`} preserveAspectRatio="none" role="img" aria-label="traffic">
      <defs>
        <linearGradient id="spark-fill" x1="0" y1="0" x2="0" y2="1">
          <stop offset="0%" stopColor="var(--primary)" stopOpacity="0.28" />
          <stop offset="100%" stopColor="var(--primary)" stopOpacity="0" />
        </linearGradient>
      </defs>
      {[0.25, 0.5, 0.75].map((fraction) => (
        <line
          key={fraction}
          x1={pad}
          x2={width - pad}
          y1={height - pad - fraction * (height - pad * 2)}
          y2={height - pad - fraction * (height - pad * 2)}
          stroke="var(--line)"
          strokeWidth="1"
          strokeDasharray="2 4"
        />
      ))}
      <path d={area} fill="url(#spark-fill)" />
      <path d={line} fill="none" stroke="var(--primary)" strokeWidth="2" strokeLinejoin="round" strokeLinecap="round" />
      <circle
        cx={(pad + (values.length - 1) * step).toFixed(1)}
        cy={(height - pad - (values.at(-1)! / max) * (height - pad * 2)).toFixed(1)}
        r="2.6"
        fill="var(--primary)"
      />
    </svg>
  );
}
