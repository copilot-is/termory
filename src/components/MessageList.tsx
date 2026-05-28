import React from "react";
import { useVirtualizer } from "@tanstack/react-virtual";
import { MessageBody } from "@/components/MessageBody";
import type { SessionMessage } from "@/types";
import { roleClass } from "@/lib/session-utils";

export function MessageList({ messages }: { messages: SessionMessage[] }) {
  const parentRef = React.useRef<HTMLDivElement>(null);
  const virtualizer = useVirtualizer({
    count: messages.length,
    getScrollElement: () => parentRef.current,
    estimateSize: () => 140,
    overscan: 6,
    measureElement: (element) => element.getBoundingClientRect().height
  });

  return (
    <div ref={parentRef} className="flex-1 overflow-auto px-4 py-2">
      <div
        style={{
          height: `${virtualizer.getTotalSize()}px`,
          width: "100%",
          position: "relative"
        }}
      >
        {virtualizer.getVirtualItems().map((virtualRow) => {
          const message = messages[virtualRow.index];
          return (
            <article
              key={virtualRow.key}
              data-index={virtualRow.index}
              ref={virtualizer.measureElement}
              data-role={roleClass(message.role)}
              style={{
                position: "absolute",
                top: 0,
                left: 0,
                width: "100%",
                transform: `translateY(${virtualRow.start}px)`,
                paddingBottom: "20px"
              }}
            >
              <header className="flex items-center gap-2 mb-1">
                <span
                  aria-hidden="true"
                  data-role={roleClass(message.role)}
                  className="w-[3px] h-[0.95em] rounded-sm shrink-0 bg-muted-foreground/60 data-[role=user]:bg-teal-500 data-[role=assistant]:bg-blue-400 data-[role=tool]:bg-amber-500"
                />
                <span className="text-xs font-medium text-muted-foreground lowercase tabular-nums">
                  {message.role || "event"}
                </span>
              </header>
              <MessageBody text={message.text} className="pl-[11px]" />
            </article>
          );
        })}
      </div>
    </div>
  );
}
