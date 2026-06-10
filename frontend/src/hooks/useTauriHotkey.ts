import { useCallback, useEffect, useMemo, useState } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { useStore } from '../store/useStore';
import { defaultBinding, formatBinding, type InputBinding } from '../utils/binding';
import { useKeyboardCapture, type HotkeyApi } from './useKeyboardCapture';

export function useTauriHotkey(onStatusMessage: (msg: string) => void): HotkeyApi {
  // Single source of truth: zustand. The desktop binding lives in Rust
  // (config file), but `useStore.shortcut` mirrors it so the in-window
  // listener in useGlobalShortcut can read the actual desktop binding.
  const binding = useStore((s) => s.shortcut);
  const setShortcut = useStore((s) => s.setShortcut);
  const [capturing, setCapturing] = useState(false);
  const [liveKeys, setLiveKeys] = useState<string[]>([]);

  // Initial load + listen for capture events from rdev. Rust currently emits
  // `input-captured` for mouse only (keyboard capture happens in the webview
  // via useKeyboardCapture and goes through `onCommit`), but we mirror any
  // payload into the store to stay source-agnostic.
  useEffect(() => {
    let unlisten: (() => void) | undefined;
    let cancelled = false;

    void (async () => {
      const { listen } = await import('@tauri-apps/api/event');

      try {
        const current = await invoke<InputBinding | null>('get_shortcut');
        if (!cancelled) setShortcut(current);
      } catch (err) {
        console.error('get_shortcut failed', err);
      }

      const off = await listen<InputBinding>('input-captured', (event) => {
        setShortcut(event.payload);
        setCapturing(false);
        onStatusMessage(`Горячая клавиша: ${formatBinding(event.payload)}`);
      });

      if (cancelled) off();
      else unlisten = off;
    })();

    return () => {
      cancelled = true;
      unlisten?.();
    };
  }, [onStatusMessage, setShortcut]);

  const cancel = useCallback(async () => {
    if (!capturing) return;
    setCapturing(false);
    try {
      await invoke('cancel_capture');
    } catch (err) {
      console.error('cancel_capture failed', err);
    }
  }, [capturing]);

  const start = useCallback(async () => {
    if (capturing) return;
    setCapturing(true);
    try {
      await invoke('start_capture');
    } catch (err) {
      console.error('start_capture failed', err);
      setCapturing(false);
    }
  }, [capturing]);

  const onCommit = useMemo(
    () => async (b: InputBinding) => {
      try {
        await Promise.all([invoke('set_shortcut', { binding: b }), invoke('cancel_capture')]);
        setShortcut(b);
        setCapturing(false);
        onStatusMessage(`Горячая клавиша: ${formatBinding(b)}`);
      } catch (err) {
        console.error('set_shortcut failed', err);
      }
    },
    [onStatusMessage, setShortcut],
  );

  useKeyboardCapture({ active: capturing, onCommit, onLiveChange: setLiveKeys });

  const clear = useCallback(async () => {
    try {
      await invoke('clear_shortcut');
      setShortcut(null);
      onStatusMessage('Горячая клавиша очищена');
    } catch (err) {
      console.error('clear_shortcut failed', err);
    }
  }, [onStatusMessage, setShortcut]);

  const reset = useCallback(async () => {
    try {
      const def = defaultBinding();
      await invoke('set_shortcut', { binding: def });
      setShortcut(def);
      onStatusMessage(`Горячая клавиша сброшена: ${formatBinding(def)}`);
    } catch (err) {
      console.error('set_shortcut failed', err);
    }
  }, [onStatusMessage, setShortcut]);

  return { binding, capturing, liveKeys, start, cancel, clear, reset };
}
