import { writable } from 'svelte/store';

export type ToastKind = 'success' | 'error';

export interface Toast {
  id: number;
  kind: ToastKind;
  text: string;
}

/** Errors linger a little longer than confirmations, but never forever. */
const LIFETIME_MS: Record<ToastKind, number> = { success: 6000, error: 10000 };

const timers = new Map<number, ReturnType<typeof setTimeout>>();
let nextId = 1;

export const toasts = writable<Toast[]>([]);

/** True when the operator asked the OS to keep motion to a minimum. */
export function reducedMotion(): boolean {
  return typeof window !== 'undefined' && window.matchMedia('(prefers-reduced-motion: reduce)').matches;
}

export function dismissToast(id: number) {
  const timer = timers.get(id);
  if (timer) {
    clearTimeout(timer);
    timers.delete(id);
  }
  toasts.update((current) => current.filter((toast) => toast.id !== id));
}

/**
 * Show a notification. Every toast is removed by its own timer, so a message can
 * never get stuck on screen even if the operator ignores it.
 */
export function pushToast(kind: ToastKind, text: string): number {
  const trimmed = text?.trim();
  if (!trimmed) return 0;
  const id = nextId;
  nextId += 1;
  // Keep the stack short; the newest message is the interesting one.
  toasts.update((current) => [...current, { id, kind, text: trimmed }].slice(-4));
  timers.set(
    id,
    setTimeout(() => dismissToast(id), LIFETIME_MS[kind])
  );
  return id;
}

export const notifySuccess = (text: string) => pushToast('success', text);
export const notifyError = (text: string) => pushToast('error', text);

export function clearToasts() {
  for (const timer of timers.values()) clearTimeout(timer);
  timers.clear();
  toasts.set([]);
}
