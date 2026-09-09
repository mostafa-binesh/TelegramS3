import { readable } from 'svelte/store';

/** The console is always mounted under this path (see Vite `base`). */
export const BASE = '/_admin';

export type ViewName = 'overview' | 'buckets' | 'transfers' | 'recovery' | 'telegram' | 'users';
export type RecoveryTab = 'issues' | 'transfers';
export type TelegramTab = 'connection' | 'proxy';

export interface Route {
  view: ViewName;
  /** Selected bucket, `buckets` view only. */
  bucket: string;
  /** Folder prefix inside the bucket, always '' or ending in '/'. */
  prefix: string;
  recoveryTab: RecoveryTab;
  telegramTab: TelegramTab;
}

const DEFAULT_ROUTE: Route = {
  view: 'overview',
  bucket: '',
  prefix: '',
  recoveryTab: 'issues',
  telegramTab: 'connection'
};

const VIEWS: ViewName[] = ['overview', 'buckets', 'transfers', 'recovery', 'telegram', 'users'];

function decode(segment: string): string {
  try {
    return decodeURIComponent(segment);
  } catch {
    // A hand-typed URL with a stray '%' should not blank the console.
    return segment;
  }
}

/**
 * Turn a browser pathname into a route. Unknown paths fall back to the
 * overview so a stale bookmark still lands somewhere useful.
 */
export function parseRoute(pathname: string): Route {
  let rest = pathname;
  if (rest.startsWith(BASE)) rest = rest.slice(BASE.length);
  const segments = rest.split('/').filter(Boolean).map(decode);
  const [first, ...tail] = segments;
  if (!first) return { ...DEFAULT_ROUTE };
  if (!VIEWS.includes(first as ViewName)) return { ...DEFAULT_ROUTE };
  const view = first as ViewName;

  if (view === 'buckets') {
    const [bucket, ...folders] = tail;
    return {
      ...DEFAULT_ROUTE,
      view,
      bucket: bucket ?? '',
      // The API wants a trailing slash on a folder prefix; the URL does not carry one.
      prefix: bucket && folders.length ? `${folders.join('/')}/` : ''
    };
  }
  if (view === 'recovery') {
    return { ...DEFAULT_ROUTE, view, recoveryTab: tail[0] === 'transfers' ? 'transfers' : 'issues' };
  }
  if (view === 'telegram') {
    return { ...DEFAULT_ROUTE, view, telegramTab: tail[0] === 'proxy' ? 'proxy' : 'connection' };
  }
  return { ...DEFAULT_ROUTE, view };
}

/** Inverse of `parseRoute`. Each segment is encoded so Unicode names survive. */
export function routePath(route: Partial<Route>): string {
  const view = route.view ?? 'overview';
  const parts: string[] = [view];
  if (view === 'buckets' && route.bucket) {
    parts.push(route.bucket);
    for (const folder of (route.prefix ?? '').split('/').filter(Boolean)) parts.push(folder);
  }
  if (view === 'recovery' && route.recoveryTab === 'transfers') parts.push('transfers');
  if (view === 'telegram' && route.telegramTab === 'proxy') parts.push('proxy');
  return `${BASE}/${parts.map(encodeURIComponent).join('/')}`;
}

export function currentRoute(): Route {
  return typeof window === 'undefined' ? { ...DEFAULT_ROUTE } : parseRoute(window.location.pathname);
}

/** Route store, kept in sync with Back/Forward and with `navigate` below. */
export const route = readable<Route>(currentRoute(), (set) => {
  if (typeof window === 'undefined') return;
  const sync = () => set(currentRoute());
  window.addEventListener('popstate', sync);
  window.addEventListener('telegrams3:navigate', sync);
  return () => {
    window.removeEventListener('popstate', sync);
    window.removeEventListener('telegrams3:navigate', sync);
  };
});

/**
 * Push (or replace) a route. `pushState` does not emit `popstate`, so we raise
 * our own event to keep the store in sync.
 */
export function navigate(next: Partial<Route>, options: { replace?: boolean } = {}) {
  if (typeof window === 'undefined') return;
  const path = routePath({ ...currentRoute(), ...next, ...normalizeView(next) });
  const target = path + window.location.search + window.location.hash;
  if (options.replace) window.history.replaceState({}, '', target);
  else if (target !== window.location.pathname + window.location.search + window.location.hash)
    window.history.pushState({}, '', target);
  else return;
  window.dispatchEvent(new Event('telegrams3:navigate'));
}

/**
 * Switching to a different view drops the previous view's sub-state, so
 * `navigate({view:'buckets'})` reliably lands on the bucket list.
 */
function normalizeView(next: Partial<Route>): Partial<Route> {
  if (!next.view || next.view === currentRoute().view) return {};
  return {
    bucket: next.bucket ?? '',
    prefix: next.prefix ?? '',
    recoveryTab: next.recoveryTab ?? 'issues',
    telegramTab: next.telegramTab ?? 'connection'
  };
}
