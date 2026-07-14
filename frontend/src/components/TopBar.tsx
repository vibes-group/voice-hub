import { AdminKeyButton } from './AdminKeyButton';
import { DesktopDownloadButton } from './DesktopDownloadButton';
import { LogoutButton } from './LogoutButton';
import { StatusPill } from './StatusPill';

export function TopBar() {
  return (
    <header
      className="flex items-center justify-between gap-4 h-14 pl-4 pr-2.5 md:pl-6
        bg-bg-1 border border-line
        max-[640px]:flex-wrap max-[640px]:h-auto max-[640px]:py-3"
    >
      <div className="flex items-center gap-2.5 min-w-0">
        <img src="/favicon.svg" alt="" className="md:hidden w-6 h-6 shrink-0" />
        <div className="text-[17px] font-extrabold uppercase tracking-[0.2em] text-accent">
          Voice&nbsp;Hub
        </div>
      </div>
      <div className="flex items-center gap-2">
        <StatusPill />
        <DesktopDownloadButton />
        <AdminKeyButton />
        <LogoutButton />
      </div>
    </header>
  );
}
