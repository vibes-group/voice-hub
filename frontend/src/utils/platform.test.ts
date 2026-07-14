import { describe, expect, it } from 'vitest';
import { isWindowsBrowser } from './platform';

describe('isWindowsBrowser', () => {
  it('prefers the user agent client hint', () => {
    expect(
      isWindowsBrowser({ userAgent: 'Mozilla/5.0', userAgentData: { platform: 'Windows' } }),
    ).toBe(true);
  });

  it('falls back to the legacy user agent', () => {
    expect(isWindowsBrowser({ userAgent: 'Mozilla/5.0 (Windows NT 10.0; Win64; x64)' })).toBe(true);
  });

  it('rejects non-Windows browsers', () => {
    expect(isWindowsBrowser({ userAgent: 'Mozilla/5.0 (X11; Linux x86_64)' })).toBe(false);
  });
});
