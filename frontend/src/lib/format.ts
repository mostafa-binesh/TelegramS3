/** Display helpers shared by the console views. */

export function formatCount(value: number) {
  return new Intl.NumberFormat('en-US').format(value);
}

export function formatBytes(value: number) {
  if (!value) return '0 B';
  const units = ['B', 'KiB', 'MiB', 'GiB', 'TiB'];
  let size = value;
  let unit = 0;
  while (size >= 1024 && unit < units.length - 1) {
    size /= 1024;
    unit += 1;
  }
  return `${size.toFixed(size >= 10 || unit === 0 ? 0 : 1)} ${units[unit]}`;
}

export function formatTimestamp(value?: string | null) {
  if (!value) return 'unknown';
  const date = new Date(value);
  return Number.isNaN(date.getTime())
    ? value
    : new Intl.DateTimeFormat('en-US', {
        dateStyle: 'medium',
        timeStyle: 'short'
      }).format(date);
}

export function normalizeError(cause: unknown) {
  return cause instanceof Error ? cause.message : 'Unexpected failure';
}
