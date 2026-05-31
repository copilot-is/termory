import React from "react";
import { StatsFilterBar } from "./StatsFilterBar";
import { OverviewHero } from "./OverviewHero";
import { DailyTokenUsageChart } from "./DailyTokenUsageChart";
import { DailyActivityHeatmap } from "./DailyActivityHeatmap";
import type { AppSession } from "@/types";
import {
  type DateRange,
  type SourceFilter,
  dailyActivity,
  dailyTokenUsage,
  filterSessions,
  resolveRange,
  windowTotals
} from "@/lib/stats-utils";

/**
 * Stats dashboard:
 *   1. StatsFilterBar    — date range + source
 *   2. OverviewHero      — KPI strip (Sessions / Messages / Tokens / Projects)
 *   3. DailyTokenUsageChart — per-day token breakdown (line chart)
 *   4. DailyActivityHeatmap — 24-hour × N-date heatmap
 */
export function StatsPage({
  sessions,
  onRefresh,
  refreshing
}: {
  sessions: AppSession[];
  onRefresh: () => void;
  refreshing: boolean;
}) {
  const [range, setRange] = React.useState<DateRange>({ preset: "30d" });
  const [source, setSource] = React.useState<SourceFilter>("All");

  const resolved = React.useMemo(() => resolveRange(range), [range]);

  const filtered = React.useMemo(
    () => filterSessions(sessions, resolved, source),
    [sessions, resolved, source]
  );

  const totals = React.useMemo(
    () => windowTotals(filtered, resolved),
    [filtered, resolved]
  );

  const tokenUsage = React.useMemo(
    () => dailyTokenUsage(filtered, resolved),
    [filtered, resolved]
  );
  const activity = React.useMemo(
    () => dailyActivity(filtered, resolved),
    [filtered, resolved]
  );

  return (
    <div className="flex-1 min-h-0 overflow-auto px-3 mt-3 pb-1">
      <div className="flex flex-col gap-3 pr-1">
        <StatsFilterBar
          range={range}
          onRangeChange={setRange}
          source={source}
          onSourceChange={setSource}
          refreshing={refreshing}
          onRefresh={onRefresh}
        />
        <OverviewHero
          sessions={totals.sessions}
          messages={totals.messages}
          tokens={totals.tokens}
          projects={totals.projects}
        />
        <DailyTokenUsageChart usage={tokenUsage} />
        <DailyActivityHeatmap activity={activity} totals={totals} />
      </div>
    </div>
  );
}
