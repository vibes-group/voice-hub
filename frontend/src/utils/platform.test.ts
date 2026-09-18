import { describe, expect, it } from 'vitest';
import { isMobileBrowser, isWindowsBrowser } from './platform';

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

describe('isMobileBrowser', () => {
  it('prefers the user agent client hint', () => {
    expect(isMobileBrowser({ userAgent: 'Mozilla/5.0', userAgentData: { mobile: true } })).toBe(
      true,
    );
  });

  it('trusts a false hint over the user agent string', () => {
    expect(
      isMobileBrowser({
        userAgent: 'Mozilla/5.0 (Linux; Android 14) Chrome/120',
        userAgentData: { mobile: false },
      }),
    ).toBe(false);
  });

  it('falls back to the legacy user agent', () => {
    expect(isMobileBrowser({ userAgent: 'Mozilla/5.0 (iPhone; CPU iPhone OS 17_0)' })).toBe(true);
    expect(isMobileBrowser({ userAgent: 'Mozilla/5.0 (Linux; Android 14) Chrome/120' })).toBe(true);
  });

  it('rejects desktop browsers', () => {
    expect(isMobileBrowser({ userAgent: 'Mozilla/5.0 (Windows NT 10.0; Win64; x64)' })).toBe(false);
  });
});
