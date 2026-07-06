// Unified input binding shape for both web and Tauri builds.
// Mirrors src-tauri/src/shortcut.rs InputBinding DTO.

import { loadShortcutRaw, saveShortcutRaw } from './storage';

export type InputBinding = { kind: 'keyboard'; keys: string[] } | { kind: 'mouse'; button: string };

export function defaultBinding(): InputBinding {
  const mac = /Mac|iPhone|iPad/i.test(navigator.platform);
  return {
    kind: 'keyboard',
    keys: mac ? ['Meta', 'Shift', 'M'] : ['Ctrl', 'Shift', 'M'],
  };
}

export function formatBinding(binding: InputBinding | null): string {
  if (!binding) return 'Not set';
  if (binding.kind === 'keyboard') return binding.keys.join(' + ');
  return `Mouse ${binding.button}`;
}

// localStorage stores `null` literal to mean "user explicitly cleared",
// distinct from missing key (first run → default).
export function loadBinding(): InputBinding | null {
  const raw = loadShortcutRaw();
  if (raw === null) {
    const def = defaultBinding();
    saveBinding(def);
    return def;
  }
  try {
    const parsed: unknown = JSON.parse(raw);
    if (parsed === null) return null;
    if (isInputBinding(parsed)) return parsed;
    return defaultBinding();
  } catch {
    return defaultBinding();
  }
}

function isInputBinding(v: unknown): v is InputBinding {
  if (typeof v !== 'object' || v === null) return false;
  const o = v as { kind?: unknown; keys?: unknown; button?: unknown };
  if (o.kind === 'keyboard') {
    return Array.isArray(o.keys) && o.keys.every((k) => typeof k === 'string');
  }
  if (o.kind === 'mouse') {
    return typeof o.button === 'string';
  }
  return false;
}

export function saveBinding(binding: InputBinding | null): void {
  saveShortcutRaw(JSON.stringify(binding));
}

// Canonical label for a modifier code (left/right collapsed).
function modifierLabelFromCode(code: string): string | null {
  switch (code) {
    case 'ControlLeft':
    case 'ControlRight':
      return 'Ctrl';
    case 'ShiftLeft':
    case 'ShiftRight':
      return 'Shift';
    case 'AltLeft':
    case 'AltRight':
      return 'Alt';
    case 'MetaLeft':
    case 'MetaRight':
      return 'Meta';
    default:
      return null;
  }
}

// Canonical label for any key code, including modifiers.
// Returns null for keys we don't have a stable mapping for (e.g. media keys
// without a corresponding rdev variant on the Rust side).
export function labelFromCode(code: string): string | null {
  const mod = modifierLabelFromCode(code);
  if (mod) return mod;
  if (code.startsWith('Key')) return code.slice(3);
  if (code.startsWith('Digit')) return code.slice(5);
  if (code.startsWith('Numpad')) {
    const tail = code.slice(6);
    switch (tail) {
      case 'Add':
        return 'Num+';
      case 'Subtract':
        return 'Num-';
      case 'Multiply':
        return 'Num*';
      case 'Divide':
        return 'Num/';
      case 'Enter':
        return 'NumEnter';
      case 'Decimal':
        return 'Num.';
      default:
        if (/^\d$/.test(tail)) return `Num${tail}`;
        return null;
    }
  }
  switch (code) {
    case 'Space':
    case 'Tab':
    case 'Enter':
    case 'Backspace':
    case 'Delete':
    case 'Escape':
    case 'Insert':
    case 'Home':
    case 'End':
    case 'PageUp':
    case 'PageDown':
    case 'CapsLock':
    case 'NumLock':
    case 'ScrollLock':
    case 'Pause':
    case 'PrintScreen':
      return code;
    case 'ArrowUp':
      return 'Up';
    case 'ArrowDown':
      return 'Down';
    case 'ArrowLeft':
      return 'Left';
    case 'ArrowRight':
      return 'Right';
    case 'Minus':
      return '-';
    case 'Equal':
      return '=';
    case 'BracketLeft':
      return '[';
    case 'BracketRight':
      return ']';
    case 'Backslash':
      return '\\';
    case 'Semicolon':
      return ';';
    case 'Quote':
      return "'";
    case 'Comma':
      return ',';
    case 'Period':
      return '.';
    case 'Slash':
      return '/';
    case 'Backquote':
      return '`';
    case 'F1':
    case 'F2':
    case 'F3':
    case 'F4':
    case 'F5':
    case 'F6':
    case 'F7':
    case 'F8':
    case 'F9':
    case 'F10':
    case 'F11':
    case 'F12':
      return code;
    default:
      return null;
  }
}

// Sort keys so modifiers come first in canonical Ctrl/Shift/Alt/Meta order,
// then the non-modifier (capture order).
export function canonicalizeKeys(keys: string[]): string[] {
  const order = ['Ctrl', 'Shift', 'Alt', 'Meta'];
  const mods = order.filter((m) => keys.includes(m));
  const rest = keys.filter((k) => !order.includes(k));
  return [...mods, ...rest];
}
