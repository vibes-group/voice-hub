interface BrowserPlatformInfo {
  userAgent: string;
  userAgentData?: {
    platform?: string;
    mobile?: boolean;
  };
}

export function isWindowsBrowser(
  browser: BrowserPlatformInfo = navigator as Navigator & BrowserPlatformInfo,
): boolean {
  return browser.userAgentData?.platform === 'Windows' || browser.userAgent.includes('Windows');
}

export function isMobileBrowser(
  browser: BrowserPlatformInfo = navigator as Navigator & BrowserPlatformInfo,
): boolean {
  if (typeof browser.userAgentData?.mobile === 'boolean') return browser.userAgentData.mobile;
  return /Android|iPhone|iPad|iPod/i.test(browser.userAgent);
}
