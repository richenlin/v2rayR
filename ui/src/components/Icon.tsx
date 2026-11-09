import type { SVGProps } from "react";

/** One stroke-based icon set, so weights match everywhere. */
export type IconName =
  | "plug"
  | "nodes"
  | "rss"
  | "gauge"
  | "sliders"
  | "sun"
  | "moon"
  | "search"
  | "star"
  | "plus"
  | "refresh"
  | "trash"
  | "copy"
  | "close"
  | "check"
  | "chevron"
  | "arrow-right"
  | "download"
  | "upload"
  | "alert"
  | "spark"
  | "clock"
  | "tag"
  | "shield"
  | "power";

const paths: Record<IconName, string> = {
  plug: "M9 3v4M15 3v4M7 7h10v3a5 5 0 0 1-5 5v0a5 5 0 0 1-5-5V7Zm5 13v-3",
  nodes: "M4 6h6M4 12h10M4 18h6m8-9 3 3-3 3m3-3h-6",
  rss: "M5 19a1 1 0 1 0 0-2 1 1 0 0 0 0 2Zm-1-6a7 7 0 0 1 7 7M4 8a11 11 0 0 1 11 11",
  gauge: "M4 17a8 8 0 1 1 16 0M12 13l4-4",
  sliders: "M4 8h9m3 0h4M4 16h4m3 0h9M15 6v4M9 14v4",
  sun: "M12 6v2m0 8v2M6 12H4m16 0h-2M7.8 7.8 6.4 6.4m11.2 11.2-1.4-1.4M7.8 16.2l-1.4 1.4M17.6 6.4l-1.4 1.4M12 9a3 3 0 1 0 0 6 3 3 0 0 0 0-6Z",
  moon: "M20 14.5A8.5 8.5 0 0 1 9.5 4a8.5 8.5 0 1 0 10.5 10.5Z",
  search: "M11 18a7 7 0 1 0 0-14 7 7 0 0 0 0 14Zm5-2 4 4",
  star: "m12 4 2.4 5 5.6.8-4 4 .9 5.6-4.9-2.7-4.9 2.7.9-5.6-4-4L9.6 9 12 4Z",
  plus: "M12 5v14M5 12h14",
  refresh: "M20 12a8 8 0 1 1-2.3-5.6M20 4v5h-5",
  trash: "M4 7h16M9 7V5h6v2m-8 0 1 13h8l1-13M10 11v6m4-6v6",
  copy: "M9 9h9v11H9V9Zm-3 6H4V4h11v2",
  close: "m6 6 12 12M18 6 6 18",
  check: "m5 13 4 4L19 7",
  chevron: "m8 10 4 4 4-4",
  "arrow-right": "M5 12h14m-6-6 6 6-6 6",
  download: "M12 4v10m0 0 4-4m-4 4-4-4M5 19h14",
  upload: "M12 14V4m0 0 4 4m-4-4L8 8M5 19h14",
  alert: "M12 4 3 20h18L12 4Zm0 5v6m0 3v.5",
  spark: "M12 4v4m0 8v4M4 12h4m8 0h4M6.5 6.5l2.5 2.5m6 6 2.5 2.5m0-11L15 9m-6 6-2.5 2.5",
  clock: "M12 20a8 8 0 1 0 0-16 8 8 0 0 0 0 16Zm0-4V8m0 0H9m3 0h3",
  tag: "M4 5h8l7 7-6 6-7-7V5Zm3 3h.01",
  shield: "M12 3 5 6v6c0 4 3 7 7 9 4-2 7-5 7-9V6l-7-3Z",
  power: "M12 4v7m5.7-4.7a8 8 0 1 1-11.4 0",
};

interface IconProps extends Omit<SVGProps<SVGSVGElement>, "name"> {
  name: IconName;
  size?: number;
}

export function Icon({ name, size = 16, strokeWidth = 1.6, ...rest }: IconProps) {
  return (
    <svg
      width={size}
      height={size}
      viewBox="0 0 24 24"
      fill="none"
      stroke="currentColor"
      strokeWidth={strokeWidth}
      strokeLinecap="round"
      strokeLinejoin="round"
      aria-hidden="true"
      focusable="false"
      {...rest}
    >
      <path d={paths[name]} />
    </svg>
  );
}

/** The mark: a stylised "M" over a mountain, matching the rail tile. */
export function BrandMark({ size = 20 }: { size?: number }) {
  return (
    <svg width={size} height={size} viewBox="0 0 24 24" fill="none" aria-hidden="true">
      <path d="M5 18V8l7 6 7-6v10" stroke="currentColor" strokeWidth="1.9" strokeLinecap="round" strokeLinejoin="round" />
      <circle cx="12" cy="19.4" r="1.05" fill="currentColor" />
    </svg>
  );
}
