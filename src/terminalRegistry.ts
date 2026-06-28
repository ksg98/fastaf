// Lets the global Cmd+W handler (registered in App, capture phase) close the
// terminal tab the user is actually focused in, instead of stowing the window.
//
// App's keydown listener runs in the capture phase on `window`, ahead of any
// handler inside the terminal panel, so the panel can't intercept Cmd+W itself.
// Each mounted ShellTerminalPanel registers a closer here; the closer is a no-op
// unless a shell terminal inside that panel currently holds focus, so with
// several panels mounted only the focused one acts.

type TerminalCloser = () => boolean;

const closers = new Set<TerminalCloser>();

/** Register a panel's "close the focused terminal" callback. Returns an unregister fn. */
export function registerTerminalCloser(closer: TerminalCloser): () => void {
  closers.add(closer);
  return () => {
    closers.delete(closer);
  };
}

/**
 * Ask the registered panels to close the terminal tab that currently has focus.
 * Returns true once one of them handled it (so the caller can skip the window-stow
 * fallback), false if no terminal was focused.
 */
export function closeFocusedTerminal(): boolean {
  for (const closer of closers) {
    if (closer()) return true;
  }
  return false;
}
