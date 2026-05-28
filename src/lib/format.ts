// `Intl.DateTimeFormat` construction is expensive (loads locale data
// the first time per option set). Cache one instance per format and
// reuse — formatDate runs hundreds of times per App re-render across
// the session list + detail messages.

const dateFormatter = new Intl.DateTimeFormat(undefined, {
  month: "short",
  day: "numeric",
  hour: "2-digit",
  minute: "2-digit"
});

const shortDateFormatter = new Intl.DateTimeFormat(undefined, {
  month: "short",
  day: "numeric"
});

const yearDateFormatter = new Intl.DateTimeFormat(undefined, {
  year: "numeric",
  month: "short",
  day: "numeric"
});

const numberFormatter = new Intl.NumberFormat();

export function formatDate(value?: string | null): string {
  if (!value) return "Unknown time";
  const date = new Date(value);
  if (Number.isNaN(date.getTime())) return value;
  return dateFormatter.format(date);
}

// Compact relative timestamp used on list cards: `2h ago`,
// `Yesterday`, `May 23`, `2024`. Drops absolute precision in
// exchange for scannability — older absolute formats stay in
// places that still need them (detail header, etc.).
export function formatRelativeDate(value?: string | null): string {
  if (!value) return "—";
  const date = new Date(value);
  if (Number.isNaN(date.getTime())) return value;
  const now = Date.now();
  const diffMs = now - date.getTime();
  const sec = Math.round(diffMs / 1000);
  if (sec < 60) return "just now";
  const min = Math.round(sec / 60);
  if (min < 60) return `${min}m ago`;
  const hr = Math.round(min / 60);
  if (hr < 24) return `${hr}h ago`;
  // Compare calendar dates for "Yesterday" / "Today" so a 23h-old
  // session at 11pm yesterday doesn't show "23h ago" forever.
  const startOfToday = new Date();
  startOfToday.setHours(0, 0, 0, 0);
  const startOfDate = new Date(date);
  startOfDate.setHours(0, 0, 0, 0);
  const dayDiff = Math.round(
    (startOfToday.getTime() - startOfDate.getTime()) / 86_400_000
  );
  if (dayDiff === 0) return `${hr}h ago`;
  if (dayDiff === 1) return "Yesterday";
  if (dayDiff < 7) return `${dayDiff}d ago`;
  if (date.getFullYear() === new Date().getFullYear()) {
    return shortDateFormatter.format(date);
  }
  return yearDateFormatter.format(date);
}

export function formatTimeAgo(timestamp: number): string {
  const sec = Math.floor((Date.now() - timestamp) / 1000);
  if (sec < 5) return "just now";
  if (sec < 60) return `${sec}s ago`;
  const min = Math.floor(sec / 60);
  if (min < 60) return `${min}m ago`;
  const hr = Math.floor(min / 60);
  if (hr < 24) return `${hr}h ago`;
  const day = Math.floor(hr / 24);
  return `${day}d ago`;
}

export function formatFullNumber(value: number): string {
  return numberFormatter.format(value);
}
