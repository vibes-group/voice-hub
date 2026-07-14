interface BrowserPlatformInfo {
  userAgent: string;
  userAgentData?: {
    platform?: string;
  };
}

export function isWindowsBrowser(
  browser: BrowserPlatformInfo = navigator as Navigator & BrowserPlatformInfo,
): boolean {
  return browser.userAgentData?.platform === 'Windows' || browser.userAgent.includes('Windows');
}
