import { type ReactNode, useMemo } from "react";
import {
  type AppConfig,
  AppConfigContext,
  configFromSettings,
  DataSourceContext,
} from "./app-context";
import type { DataSource } from "./data-source";
import { useSettings } from "./queries";
import { ToastProvider } from "./toasts";

interface AppProviderProps {
  /** Used until the settings query answers, and kept for whatever settings do not carry. */
  readonly config: AppConfig;
  readonly dataSource: DataSource;
  readonly children: ReactNode;
}

/**
 * Data source, config and toast region for one window. Must sit inside a `QueryClientProvider`:
 * the config follows the `settings` query, so the window renders at once with `config` and picks
 * up the display zone and currency when they arrive or change (settings refetch after actions
 * and full store refreshes).
 */
export function AppProvider({ config, dataSource, children }: AppProviderProps): ReactNode {
  return (
    <DataSourceContext value={dataSource}>
      <SettingsConfig base={config}>
        <ToastProvider>{children}</ToastProvider>
      </SettingsConfig>
    </DataSourceContext>
  );
}

function SettingsConfig(props: { readonly base: AppConfig; readonly children: ReactNode }) {
  const { base, children } = props;
  const settings = useSettings().data;
  const config = useMemo(
    () => (settings === undefined ? base : configFromSettings(base, settings)),
    [base, settings],
  );
  return <AppConfigContext value={config}>{children}</AppConfigContext>;
}
