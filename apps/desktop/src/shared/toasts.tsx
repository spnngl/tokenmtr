import { CircleAlert, CircleCheck, X } from "lucide-react";
import { createContext, type ReactNode, useContext, useState } from "react";
import {
  Button as AriaButton,
  Text,
  UNSTABLE_Toast as Toast,
  UNSTABLE_ToastContent as ToastContent,
  UNSTABLE_ToastQueue as ToastQueue,
  UNSTABLE_ToastRegion as ToastRegion,
} from "react-aria-components";

/** The outcome of a user action, announced in the toast region. */
export interface Notice {
  readonly tone: "success" | "error";
  readonly title: string;
  readonly message?: string;
}

/** Success notices leave on their own; errors stay until dismissed so they cannot be missed. */
const SUCCESS_TIMEOUT_MS = 5_000;

export type Notify = (notice: Notice) => void;

const NotifyContext = createContext<Notify | null>(null);

const TONE_ICON = {
  success: <CircleCheck className="mt-0.5 size-4 shrink-0 text-success" aria-hidden />,
  error: <CircleAlert className="mt-0.5 size-4 shrink-0 text-danger" aria-hidden />,
} as const;

/**
 * The window's one toast region (React Aria: a landmark, announced, F6 reaches it). Every action's
 * outcome goes through it, so feedback survives the control that triggered it (a closed palette,
 * a recap window that hid itself).
 */
export function ToastProvider({ children }: { readonly children: ReactNode }): ReactNode {
  const [queue] = useState(() => new ToastQueue<Notice>({ maxVisibleToasts: 3 }));
  const [notify] = useState<Notify>(() => (notice: Notice) => {
    queue.add(notice, notice.tone === "success" ? { timeout: SUCCESS_TIMEOUT_MS } : {});
  });
  return (
    <NotifyContext value={notify}>
      {children}
      <ToastRegion
        queue={queue}
        aria-label="Notifications"
        className="fixed right-4 bottom-4 z-50 flex w-[min(24rem,calc(100vw-2rem))] flex-col gap-2"
      >
        {({ toast }) => (
          <Toast
            toast={toast}
            className="flex items-start gap-3 rounded-md border border-border bg-surface-raised p-3 text-sm shadow-lg"
          >
            {TONE_ICON[toast.content.tone]}
            <ToastContent className="flex min-w-0 flex-1 flex-col gap-0.5">
              <Text slot="title" className="font-semibold">
                {toast.content.title}
              </Text>
              {toast.content.message === undefined ? null : (
                <Text slot="description" className="break-words text-fg-muted">
                  {toast.content.message}
                </Text>
              )}
            </ToastContent>
            <AriaButton
              slot="close"
              aria-label="Dismiss"
              className="-m-1 rounded-md p-1 text-fg-muted data-hovered:bg-surface-muted data-hovered:text-fg"
            >
              <X className="size-4" aria-hidden />
            </AriaButton>
          </Toast>
        )}
      </ToastRegion>
    </NotifyContext>
  );
}

/** @throws {Error} outside a `ToastProvider` (every `AppProvider` renders one). */
export function useNotify(): Notify {
  const notify = useContext(NotifyContext);
  if (notify === null) {
    throw new Error("useNotify must be used inside ToastProvider");
  }
  return notify;
}
