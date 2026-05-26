export async function copyToClipboard(text: string): Promise<void> {
  // navigator.clipboard is available in the Tauri webview; failure
  // is silently swallowed since there's no useful recovery (clipboard
  // permissions in webviews don't get prompted, they're granted).
  try {
    await navigator.clipboard.writeText(text);
  } catch (err) {
    console.warn("clipboard write failed", err);
  }
}
