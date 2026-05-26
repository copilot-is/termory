export type SnippetSegment = { text: string; match: boolean };

export function splitSnippet(snippet: string, query: string): SnippetSegment[] {
  if (!query) return [{ text: snippet, match: false }];
  const lowerSnippet = snippet.toLowerCase();
  const lowerQuery = query.toLowerCase();
  const out: SnippetSegment[] = [];
  let cursor = 0;
  while (cursor < snippet.length) {
    const idx = lowerSnippet.indexOf(lowerQuery, cursor);
    if (idx === -1) {
      out.push({ text: snippet.slice(cursor), match: false });
      break;
    }
    if (idx > cursor) out.push({ text: snippet.slice(cursor, idx), match: false });
    out.push({ text: snippet.slice(idx, idx + lowerQuery.length), match: true });
    cursor = idx + lowerQuery.length;
  }
  return out;
}
