import { TriangleAlert } from "lucide-react";
import type { ReactNode } from "react";
import { APP_NAME } from "./brand";
import { failureKind } from "./errors";

/**
 * What to do when installing the background service or the login item failed with
 * `app_not_installed`: the app runs from a disk image or a translocated copy, which launchd must
 * not point at. Renders nothing for any other outcome, so it sits under the control unconditionally
 * and is fed the control's last error (`null` before any failure).
 */
export function MoveToApplicationsHint({ error }: { readonly error: unknown }): ReactNode {
  if (failureKind(error) !== "app_not_installed") {
    return null;
  }
  return (
    <p className="flex items-start gap-2 text-sm">
      <TriangleAlert className="mt-0.5 size-4 shrink-0 text-warning" aria-hidden />
      Move {APP_NAME} to Applications, then open it from there.
    </p>
  );
}
