import { Skeleton } from "@/components/ui/skeleton";

/**
 * Skeleton placeholder that mirrors the session card layout used in
 * the Records list. Shown while the initial scan is in flight so the
 * pane has visual structure (where the title / date / project bar
 * will appear) rather than a centered spinner. The shimmer + colour
 * come from the shadcn `<Skeleton>` primitive.
 */
export function SessionCardSkeleton({
  // Slight per-card variability avoids the "list of identical bars"
  // robotic look while keeping each card the same overall shape.
  titleWidth = "70%",
  projectWidth = "8rem"
}: {
  titleWidth?: string;
  projectWidth?: string;
}) {
  return (
    <div
      aria-hidden
      className="w-full rounded-lg px-2 py-2 flex flex-col gap-2"
    >
      <div className="flex items-baseline justify-between gap-2">
        <Skeleton className="h-4" style={{ width: titleWidth }} />
        <Skeleton className="h-3 w-10 shrink-0" />
      </div>
      <div className="flex items-center justify-between gap-2">
        <Skeleton className="h-3" style={{ width: projectWidth }} />
        <Skeleton className="h-3 w-12 shrink-0" />
      </div>
    </div>
  );
}

/**
 * Convenience: renders N skeletons with mildly varied widths so the
 * list reads as "loading content" rather than a stamp pattern.
 */
export function SessionCardSkeletonList({ count = 6 }: { count?: number }) {
  const widths = [
    { titleWidth: "68%", projectWidth: "9rem" },
    { titleWidth: "82%", projectWidth: "7rem" },
    { titleWidth: "55%", projectWidth: "11rem" },
    { titleWidth: "74%", projectWidth: "8rem" },
    { titleWidth: "60%", projectWidth: "10rem" },
    { titleWidth: "78%", projectWidth: "6rem" }
  ];
  return (
    <>
      {Array.from({ length: count }, (_, i) => (
        <SessionCardSkeleton key={i} {...widths[i % widths.length]} />
      ))}
    </>
  );
}
