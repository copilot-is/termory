import React from "react";
import { Card, CardContent } from "@/components/ui/card";
import {
  HoverCard,
  HoverCardContent,
  HoverCardTrigger
} from "@/components/ui/hover-card";
import { cn } from "@/lib/utils";
import { formatCompact, formatFullNumber } from "@/lib/format";
import type { DailyActivity, WindowTotals } from "@/lib/stats-utils";
import { formatDateLong, formatDateShort } from "./shared";

/**
 * Daily workload heatmap — 24-hour rows × N-date columns, CSS Grid
 * with `1fr` columns so the date axis auto-stretches.
 *
 * Cells with no activity are inert: no hover effect, no hover card.
 * Active cells get a thin outline on hover (ring, not background
 * change) and reveal a shadcn HoverCard with sessions / messages /
 * tokens for that exact (date, hour) bucket — no day-level mixing.
 *
 * All hour labels and tooltip times use the 24-hour clock.
 */

// Hand-picked label rows: sparse at night, dense across the work
// band (09–12 morning, 14–18 afternoon — 13 dropped as lunch).
const LABEL_HOURS = new Set([0, 3, 6, 9, 10, 11, 12, 14, 15, 16, 17, 18, 21, 23]);

function hourLabel(h: number): string {
  if (!LABEL_HOURS.has(h)) return "";
  // 24-hour scale convention: just the hour, 2-digit padded. The
  // HoverCard preserves the full `HH:00 – HH:00` form when an exact
  // time-range is needed.
  return String(h).padStart(2, "0");
}

function formatHourRange24(h: number): string {
  const next = (h + 1) % 24;
  return `${String(h).padStart(2, "0")}:00 – ${String(next).padStart(2, "0")}:00`;
}

function cellClass(value: number, max: number): string {
  if (value === 0 || max === 0) return "bg-foreground/[0.04]";
  const ratio = value / max;
  if (ratio < 0.25) return "bg-primary/25";
  if (ratio < 0.5) return "bg-primary/45";
  if (ratio < 0.75) return "bg-primary/65";
  return "bg-primary/90";
}

export function DailyActivityHeatmap({
  activity,
  totals
}: {
  activity: DailyActivity;
  totals: WindowTotals;
}) {
  const { dates, messages, tokens, sessions } = activity;

  const max = React.useMemo(() => {
    let m = 0;
    for (let h = 0; h < 24; h++) {
      for (let d = 0; d < dates.length; d++) {
        if (messages[h][d] > m) m = messages[h][d];
      }
    }
    return m;
  }, [messages, dates.length]);
  const hasAnyActivity =
    totals.sessions > 0 || totals.messages > 0 || totals.tokens.total > 0;

  // Date labels — anchor on last, every 2 days backwards.
  const dateTickIndices = React.useMemo(() => {
    const set = new Set<number>();
    for (let i = dates.length - 1; i >= 0; i -= 2) set.add(i);
    return set;
  }, [dates.length]);

  return (
    <Card className="p-3 gap-2 outline outline-1 outline-transparent bg-card shadow-sm">
      <CardContent className="px-0 flex flex-col gap-3">
        <div className="flex items-baseline justify-between gap-2 flex-wrap">
          <h3 className="text-sm font-medium text-muted-foreground uppercase tracking-wide">
            Daily activity
          </h3>
          {hasAnyActivity && (
            <span className="text-[11px] text-muted-foreground tabular-nums">
              {totals.sessions} sessions · {totals.messages.toLocaleString()} messages ·{" "}
              <span title={formatFullNumber(totals.tokens.total)}>
                {totals.tokens.total === 0
                  ? "—"
                  : formatCompact(totals.tokens.total)} tokens
              </span>
            </span>
          )}
        </div>
        {!hasAnyActivity ? (
          <div className="h-[220px] flex items-center justify-center text-sm text-muted-foreground">
            No activity in this range.
          </div>
        ) : (
          <div
            className="grid w-full gap-1"
            style={{
              gridTemplateColumns: `42px repeat(${dates.length}, 1fr)`
            }}
          >
            {Array.from({ length: 24 }, (_, h) => (
              <React.Fragment key={`row-${h}`}>
                <div
                  className={cn(
                    "text-[9px] tabular-nums leading-none flex items-center justify-end pr-2",
                    // Highlight working hours (09:00–18:00) with the
                    // full foreground tone; others stay muted.
                    h >= 9 && h <= 18
                      ? "text-foreground"
                      : "text-muted-foreground"
                  )}
                >
                  {hourLabel(h)}
                </div>
                {dates.map((d, i) => {
                  const msgCount = messages[h][i];
                  const sessCount = sessions[h][i];
                  const tokCount = tokens[h][i];
                  // Inert cell only when NOTHING happened — no messages
                  // AND no session was created in this hour. The
                  // sessions-only case (started at 22:55, first message
                  // at 23:00) should still be hoverable.
                  if (msgCount === 0 && sessCount === 0) {
                    return (
                      <div
                        key={`${h}-${i}`}
                        className={cn("h-[10px]", cellClass(0, max))}
                      />
                    );
                  }
                  // Intensity scales off messages; a sessions-only cell
                  // would otherwise render the inert color, so floor it
                  // at the lightest active tier.
                  const colorClass =
                    msgCount === 0 ? "bg-primary/25" : cellClass(msgCount, max);
                  return (
                    <HoverCard
                      key={`${h}-${i}`}
                      openDelay={50}
                      closeDelay={80}
                    >
                      <HoverCardTrigger asChild>
                        <div
                          className={cn(
                            "h-[10px] cursor-default transition-shadow duration-75",
                            colorClass,
                            "hover:ring-1 hover:ring-foreground/50 hover:relative hover:z-10"
                          )}
                        />
                      </HoverCardTrigger>
                      <HoverCardContent
                        className="w-auto p-3 text-xs leading-tight"
                        side="top"
                        align="center"
                      >
                        <div className="font-medium pb-1.5 mb-1.5 border-b border-border/40">
                          {formatDateLong(d)} · {formatHourRange24(h)}
                        </div>
                        <div className="space-y-0.5 tabular-nums">
                          <div className="flex items-center gap-2">
                            <span className="text-muted-foreground w-20">
                              Sessions
                            </span>
                            <span>{sessCount}</span>
                          </div>
                          <div className="flex items-center gap-2">
                            <span className="text-muted-foreground w-20">
                              Messages
                            </span>
                            <span>{msgCount}</span>
                          </div>
                          <div className="flex items-center gap-2">
                            <span className="text-muted-foreground w-20">
                              Tokens
                            </span>
                            <span title={formatFullNumber(tokCount)}>
                              {tokCount === 0 ? "—" : formatCompact(tokCount)}
                            </span>
                          </div>
                        </div>
                      </HoverCardContent>
                    </HoverCard>
                  );
                })}
              </React.Fragment>
            ))}
            <div />
            {dates.map((d, i) =>
              dateTickIndices.has(i) ? (
                <div
                  key={`dl-${i}`}
                  className="text-[9px] text-muted-foreground tabular-nums leading-none pt-1 text-center"
                >
                  {formatDateShort(d)}
                </div>
              ) : (
                <div key={`dl-${i}`} />
              )
            )}
          </div>
        )}
        <div className="flex justify-end items-center gap-1 text-[10px] text-muted-foreground">
          <span>Less</span>
          <span className="size-2.5 bg-foreground/[0.04]" />
          <span className="size-2.5 bg-primary/25" />
          <span className="size-2.5 bg-primary/45" />
          <span className="size-2.5 bg-primary/65" />
          <span className="size-2.5 bg-primary/90" />
          <span>More</span>
        </div>
      </CardContent>
    </Card>
  );
}
