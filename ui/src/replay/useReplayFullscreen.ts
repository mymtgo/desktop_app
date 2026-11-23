import { getCurrentWindow } from '@tauri-apps/api/window';
import { onMounted, onUnmounted, readonly, shallowRef } from 'vue';

type ReplayWindow = ReturnType<typeof getCurrentWindow>;

/**
 * Puts the replay window into fullscreen, dropping the desktop's chrome so
 * the board gets the whole screen. v1 uses the window's own fullscreen
 * (replays spec ruling 8), since a webview's element fullscreen differs
 * between WebView2 and WebKitGTK. A compositor may refuse fullscreen or leave
 * it on its own, so the state shown is always read back from the window:
 * after asking, after every resize and on focus (spec section 6.1).
 */
export function useReplayFullscreen() {
    const supported = shallowRef(false);
    const active = shallowRef(false);
    let current: ReplayWindow | null = null;
    let stopped = false;
    const unlisten: (() => void)[] = [];

    async function sync() {
        if (!current) {
            return;
        }

        try {
            active.value = await current.isFullscreen();
        } catch {
            // The window is closing; the state no longer matters.
        }
    }

    async function request(on: boolean) {
        if (!current) {
            return;
        }

        try {
            await current.setFullscreen(on);
        } catch {
            // Refused: the read below keeps the button as the window is.
        }

        await sync();
    }

    async function toggle() {
        if (!supported.value) {
            return;
        }

        await request(!active.value);
    }

    /**
     * Esc while fullscreen leaves fullscreen and goes no further, as the
     * browser did for element fullscreen; true when it did.
     */
    function leave(): boolean {
        if (!active.value) {
            return false;
        }

        void request(false);

        return true;
    }

    async function keep(listening: Promise<() => void>) {
        const stop = await listening;

        if (stopped) {
            stop();
        } else {
            unlisten.push(stop);
        }
    }

    onMounted(async () => {
        try {
            current = getCurrentWindow();
        } catch {
            return;
        }

        supported.value = true;

        try {
            await Promise.all([keep(current.onResized(() => void sync())), keep(current.onFocusChanged(() => void sync()))]);
        } catch {
            // Without events the state is still read after each request.
        }

        await sync();
    });

    onUnmounted(() => {
        stopped = true;
        unlisten.splice(0).forEach((stop) => stop());
        current = null;
    });

    return { supported: readonly(supported), active: readonly(active), toggle, leave };
}
