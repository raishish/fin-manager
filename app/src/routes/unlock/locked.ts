// The "Locked" answer and what to do about it (T5.12). Pure, so `node --test` can check it
// without a window: see locked.test.ts.

/**
 * Whether a command's error means the ledger is locked: the unlock gate's answer to any
 * command while locked (`"locked"`), or a command's own `{ code: "Locked" }`.
 */
export function isLockedError(e: unknown): boolean {
	if (e === 'locked') return true;
	return (
		typeof e === 'object' && e !== null && 'code' in e && (e as { code: unknown }).code === 'Locked'
	);
}

/** Pages that work without an open ledger, so a Locked answer there isn't news. */
export function worksWhileLocked(path: string): boolean {
	if (path === '/backup/restore' || path === '/backup/restore/') return true;
	return (path === '/unlock' || path.startsWith('/unlock/')) && !path.startsWith('/unlock/settings');
}

type Invoke = (...args: never[]) => Promise<unknown>;

/**
 * Wraps the IPC invoke so a Locked answer to *any* command sends the UI to /unlock (via
 * `onLocked`), even if the core's lock event was missed. The caller still gets the error.
 */
export function guardInvoke<F extends Invoke>(
	inner: F,
	onLocked: () => void,
	currentPath: () => string
): F {
	const wrapped = async (...args: Parameters<F>) => {
		try {
			return await inner(...args);
		} catch (e) {
			if (isLockedError(e) && !worksWhileLocked(currentPath())) onLocked();
			throw e;
		}
	};
	return wrapped as unknown as F;
}
