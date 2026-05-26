import React from "react";
import { Check, Copy } from "lucide-react";
import { Button } from "@/components/ui/button";
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuTrigger
} from "@/components/ui/dropdown-menu";
import { copyToClipboard } from "@/lib/clipboard";

export function CopyMenu({ items }: { items: { label: string; value: string }[] }) {
  const [copied, setCopied] = React.useState<string | null>(null);

  const handleCopy = async (label: string, value: string) => {
    await copyToClipboard(value);
    setCopied(label);
    window.setTimeout(() => setCopied(null), 1200);
  };

  return (
    <DropdownMenu>
      <DropdownMenuTrigger asChild>
        <Button
          variant="ghost"
          size="icon"
          className="size-7"
          title="Copy…"
          aria-label="Copy…"
        >
          <Copy size={14} />
        </Button>
      </DropdownMenuTrigger>
      <DropdownMenuContent align="end" className="min-w-[180px]">
        {items.map((item) => (
          <DropdownMenuItem
            key={item.label}
            onClick={(e) => {
              e.preventDefault();
              void handleCopy(item.label, item.value);
            }}
            className="flex items-center justify-between gap-3"
          >
            <span>{item.label}</span>
            {copied === item.label && <Check size={12} className="text-primary" />}
          </DropdownMenuItem>
        ))}
      </DropdownMenuContent>
    </DropdownMenu>
  );
}
