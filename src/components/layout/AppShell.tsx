import { useCallback, useEffect, useState } from "react";
import { Settings } from "@solar-icons/react";
import { SettingsModal } from "./SettingsModal";
import { WorkspaceMenu } from "./WorkspaceMenu";
import { OnboardingModal } from "@/components/onboarding/OnboardingModal";
import { TranscriptionProvider } from "@/context/TranscriptionContext";
import { checkForUpdates } from "@/lib/updater";
import { isElectron } from "@/lib/desktop";

interface AppShellProps {
  children: React.ReactNode;
}

const ONBOARDING_COMPLETE_KEY = "listenos_onboarding_complete";

function AppShellContent({ children }: AppShellProps) {
  const [isSettingsOpen, setIsSettingsOpen] = useState(false);
  const [isOnboardingOpen, setIsOnboardingOpen] = useState(false);
  const [isWindowMaximized, setIsWindowMaximized] = useState(false);
  const electronDesktop = isElectron();

  useEffect(() => {
    checkForUpdates(true);
  }, []);

  useEffect(() => {
    if (!electronDesktop) return;
    void window.listenOS?.window.isMaximized().then(setIsWindowMaximized);
  }, [electronDesktop]);

  useEffect(() => {
    if (!electronDesktop) return;
    try {
      setIsOnboardingOpen(localStorage.getItem(ONBOARDING_COMPLETE_KEY) !== "true");
    } catch {
      setIsOnboardingOpen(true);
    }
  }, [electronDesktop]);

  const handleOnboardingComplete = useCallback(() => {
    try {
      localStorage.setItem(ONBOARDING_COMPLETE_KEY, "true");
    } catch {
      // The session can still continue if browser storage is unavailable.
    }
    setIsOnboardingOpen(false);
  }, []);

  const handleWindowMinimize = useCallback(() => {
    if (!electronDesktop) return;
    void window.listenOS?.window.minimize().catch((error) => {
      console.error("Failed to minimize window:", error);
    });
  }, [electronDesktop]);

  const handleWindowToggleMaximize = useCallback(() => {
    if (!electronDesktop) return;
    void window.listenOS?.window.toggleMaximize().then(setIsWindowMaximized).catch((error) => {
      console.error("Failed to toggle window size:", error);
    });
  }, [electronDesktop]);

  const handleWindowClose = useCallback(() => {
    if (!electronDesktop) return;
    void window.listenOS?.window.close().catch((error) => {
      console.error("Failed to close window:", error);
    });
  }, [electronDesktop]);

  return (
    <>
      <main className="min-h-screen bg-background">
        <header className="sticky top-0 z-50 border-b border-border bg-background/95 backdrop-blur-xl">
          <div className="desktop-drag-region flex h-14 items-center gap-3 px-4 sm:px-6">
            <div className="desktop-no-drag flex min-w-0 items-center gap-3">
              <div className="flex shrink-0 items-center gap-2.5">
                <img src="/logo.svg" alt="ListenOS" width={30} height={30} className="h-7 w-7 rounded-df" />
                <span className="hidden text-sm font-medium text-foreground sm:inline">ListenOS</span>
              </div>
              <span aria-hidden="true" className="text-base text-muted-foreground/70">/</span>
              <WorkspaceMenu />
            </div>

            <div className="flex-1" />

            <div className="desktop-no-drag flex shrink-0 items-center">
              <button
                type="button"
                onClick={() => setIsSettingsOpen(true)}
                className="ui-button ui-button-ghost flex h-9 items-center gap-2 rounded-df px-2.5 text-sm text-muted-foreground transition-colors hover:text-foreground sm:px-3"
                aria-label="Open settings"
              >
                <Settings size={18} weight="Bold" />
                <span className="hidden sm:inline">Settings</span>
              </button>

              {electronDesktop && (
                <div className="ml-2 flex border-l border-border pl-2">
                  <button
                    type="button"
                    onClick={handleWindowMinimize}
                    aria-label="Minimize window"
                    className="ui-button ui-button-ghost grid h-9 w-9 place-items-center text-muted-foreground hover:text-foreground"
                  >
                    <svg className="h-3.5 w-3.5" fill="none" stroke="currentColor" viewBox="0 0 24 24">
                      <path strokeLinecap="round" strokeLinejoin="round" strokeWidth={2} d="M6 12h12" />
                    </svg>
                  </button>
                  <button
                    type="button"
                    onClick={handleWindowToggleMaximize}
                    aria-label={isWindowMaximized ? "Restore window" : "Maximize window"}
                    className="ui-button ui-button-ghost grid h-9 w-9 place-items-center text-muted-foreground hover:text-foreground"
                  >
                    {isWindowMaximized ? (
                      <svg className="h-3.5 w-3.5" fill="none" stroke="currentColor" viewBox="0 0 24 24">
                        <rect x="8" y="8" width="10" height="10" rx="1" strokeWidth={2} />
                        <path strokeLinecap="round" strokeLinejoin="round" strokeWidth={2} d="M6 16V6h10" />
                      </svg>
                    ) : (
                      <svg className="h-3.5 w-3.5" fill="none" stroke="currentColor" viewBox="0 0 24 24">
                        <rect x="6" y="6" width="12" height="12" rx="1" strokeWidth={2} />
                      </svg>
                    )}
                  </button>
                  <button
                    type="button"
                    onClick={handleWindowClose}
                    aria-label="Close window"
                    className="ui-button grid h-9 w-9 place-items-center text-muted-foreground transition-colors hover:bg-negative/10 hover:text-negative"
                  >
                    <svg className="h-3.5 w-3.5" fill="none" stroke="currentColor" viewBox="0 0 24 24">
                      <path strokeLinecap="round" strokeLinejoin="round" strokeWidth={2} d="M6 6l12 12M18 6L6 18" />
                    </svg>
                  </button>
                </div>
              )}
            </div>
          </div>
        </header>

        <div className="px-4 py-6 sm:px-8 sm:py-7">
          <div className="mx-auto w-full max-w-5xl">{children}</div>
        </div>
      </main>

      <SettingsModal isOpen={isSettingsOpen} onClose={() => setIsSettingsOpen(false)} />
      <OnboardingModal isOpen={isOnboardingOpen} onComplete={handleOnboardingComplete} />
    </>
  );
}

export function AppShell({ children }: AppShellProps) {
  return (
    <TranscriptionProvider>
      <AppShellContent>{children}</AppShellContent>
    </TranscriptionProvider>
  );
}
