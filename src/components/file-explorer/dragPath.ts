// MIME type used to tag an internal file-tree drag so drop targets can tell our
// drag apart from arbitrary dropped text. Drop handlers read this first, falling
// back to "text/plain".
export const FASTAF_FILE_PATH_MIME = "application/x-fastaf-file-path";

// Quote a path for safe insertion into a POSIX shell command line.
export function shellQuotePath(p: string): string {
  if (/^[A-Za-z0-9_./-]+$/.test(p)) return p; // no quoting needed
  return `'${p.replace(/'/g, `'\\''`)}'`; // single-quote, escape embedded quotes
}
