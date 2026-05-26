import React from "react";
import ReactMarkdown from "react-markdown";
import remarkGfm from "remark-gfm";
import { cn } from "@/lib/utils";

const messageRemarkPlugins = [remarkGfm];

export const MessageBody = React.memo(function MessageBody({
  text,
  className
}: {
  text: string;
  className?: string;
}) {
  return (
    <div className={cn("message-body", className)}>
      <ReactMarkdown remarkPlugins={messageRemarkPlugins}>
        {text}
      </ReactMarkdown>
    </div>
  );
});
