import React from "react";

export function TimeSeparator({ timestamp }: { timestamp?: string }) {
  const label = React.useMemo(() => {
    if (!timestamp) return "";
    const date = new Date(timestamp);
    if (isNaN(date.getTime())) return "";
    // HH:MM in the user's locale — short, parses at a glance.
    return date.toLocaleTimeString([], { hour: "2-digit", minute: "2-digit" });
  }, [timestamp]);
  return (
    <div
      aria-hidden={!label}
      title={timestamp}
      className="flex items-center justify-center text-[10.5px] uppercase tracking-wide text-muted-foreground/80"
    >
      <span className="px-2 bg-background">{label}</span>
    </div>
  );
}
