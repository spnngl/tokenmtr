import type { ReactNode } from "react";
import { Switch } from "react-aria-components";
import {
  useDataAction,
  useInstallDaemon,
  useRestartDaemon,
  useRevealPath,
  useScanNow,
} from "../../shared/actions";
import { useAppConfig } from "../../shared/app-context";
import { Button } from "../../shared/button";
import { formatDateTime } from "../../shared/format";
import { useSettings } from "../../shared/queries";
import { RemoteView } from "../../shared/remote-view";
import type { Settings } from "../../shared/schemas";
import { ThemeSwitcher } from "../../shared/theme-switcher";
import { Card, PageHeader } from "../../shared/ui";
import { toRemote } from "../../shared/use-remote";
import { AgentsSection } from "./agents-section";
import {
  BackupsSection,
  EraseSection,
  ParseIssuesSection,
  ProtectedFoldersSection,
  RecentAlertsSection,
} from "./data-section";

const DAEMON_TEXT: Record<Settings["daemon"]["state"], string> = {
  running: "Running",
  stale: "Not responding",
  never_started: "Never started",
};

function Fact(props: { readonly label: string; readonly children: ReactNode }): ReactNode {
  return (
    <div className="flex flex-col gap-0.5 sm:flex-row sm:gap-3">
      <dt className="w-44 shrink-0 text-sm text-fg-muted">{props.label}</dt>
      <dd className="min-w-0 break-words text-sm">{props.children}</dd>
    </div>
  );
}

function GeneralCard({ settings }: { readonly settings: Settings }): ReactNode {
  const reveal = useRevealPath();
  const revealing = (path: string): boolean => reveal.isPending && reveal.variables === path;
  const login = useDataAction((ds, enabled: boolean) => ds.setLaunchAtLogin(enabled), {
    failure: "Could not change launch at login",
  });
  return (
    <Card title="General">
      <dl className="flex flex-col gap-2">
        <Fact label="Config file">
          <code>{settings.config_path}</code>
        </Fact>
        <Fact label="Machine">{settings.machine}</Fact>
        <Fact label="Exports folder">
          <code>{settings.exports_dir}</code>
        </Fact>
      </dl>
      <div className="flex flex-wrap gap-2">
        <Button
          isPending={revealing(settings.config_path)}
          onPress={() => reveal.mutate(settings.config_path)}
        >
          Reveal config file
        </Button>
        <Button
          isPending={revealing(settings.exports_dir)}
          onPress={() => reveal.mutate(settings.exports_dir)}
        >
          Reveal exports folder
        </Button>
      </div>
      <Switch
        // Shows the requested state while it is applied; a failure toasts and falls back.
        isSelected={login.isPending ? login.variables : settings.launch_at_login}
        onChange={(enabled) => login.mutate(enabled)}
        className="group flex min-h-9 w-fit items-center gap-2 text-sm"
      >
        <span className="flex h-5 w-9 items-center rounded-full border border-border bg-surface-muted px-0.5 transition-colors group-data-selected:bg-accent group-data-focus-visible:outline-2 group-data-focus-visible:outline-offset-2 group-data-focus-visible:outline-ring">
          <span className="size-4 rounded-full bg-fg-muted transition-transform group-data-selected:translate-x-4 group-data-selected:bg-accent-fg" />
        </span>
        Launch at login
      </Switch>
    </Card>
  );
}

function DaemonCard({ settings }: { readonly settings: Settings }): ReactNode {
  const { timeZone } = useAppConfig();
  const { daemon } = settings;
  const scan = useScanNow();
  const restart = useRestartDaemon();
  const install = useInstallDaemon();
  return (
    <Card title="Daemon">
      <dl className="flex flex-col gap-2">
        <Fact label="Status">{DAEMON_TEXT[daemon.state]}</Fact>
        <Fact label="Version">{daemon.version}</Fact>
        <Fact label="Last heartbeat">
          {daemon.last_heartbeat === null
            ? "Never"
            : formatDateTime(Date.parse(daemon.last_heartbeat), timeZone)}
        </Fact>
      </dl>
      <div className="flex flex-wrap gap-2">
        <Button isPending={scan.isPending} onPress={() => scan.mutate()}>
          Scan now
        </Button>
        {daemon.state === "running" ? (
          <Button isPending={restart.isPending} onPress={() => restart.mutate()}>
            Restart daemon
          </Button>
        ) : (
          <Button variant="primary" isPending={install.isPending} onPress={() => install.mutate()}>
            Install / start daemon
          </Button>
        )}
      </div>
    </Card>
  );
}

function DisplayCard({ settings }: { readonly settings: Settings }): ReactNode {
  const { fx, display, pricing } = settings;
  return (
    <Card title="Currency and pricing">
      <dl className="flex flex-col gap-2">
        <Fact label="Display currency">{display.currency}</Fact>
        <Fact label="Time zone">{display.time_zone}</Fact>
        <Fact label="EUR per USD">
          {fx.fixed_eur_per_usd !== null
            ? `${fx.fixed_eur_per_usd} (fixed in config)`
            : fx.latest === null
              ? "No rate yet: EUR amounts show in USD"
              : `${(fx.latest.eur_per_usd_scaled / 1e9).toFixed(4)} (${fx.latest.source}, ${fx.latest.date})`}
        </Fact>
        {fx.offline ? (
          <Fact label="Rate source">
            Offline: the last known rate is used and EUR amounts are approximate
          </Fact>
        ) : null}
        <Fact label="Pricing snapshot">
          {pricing.snapshot_date} ({pricing.models} models)
        </Fact>
      </dl>
    </Card>
  );
}

function PreferencesCard({ settings }: { readonly settings: Settings }): ReactNode {
  const { tray, alerts } = settings;
  const percents = (values: readonly number[]): string =>
    values.length === 0 ? "None" : values.map((value) => `${value}%`).join(", ");
  return (
    <Card title="Preferences">
      <dl className="flex flex-col gap-2">
        <Fact label="Menu-bar title">{tray.title}</Fact>
        <Fact label="Global shortcut">
          <kbd>{tray.shortcut}</kbd>
        </Fact>
        <Fact label="Quota alerts at">{percents(alerts.quota_thresholds)}</Fact>
        <Fact label="Budget alerts at">{percents(alerts.budget_thresholds)}</Fact>
        <Fact label="Alert on quota reset">{alerts.quota_reset ? "Yes" : "No"}</Fact>
      </dl>
      <ThemeSwitcher />
    </Card>
  );
}

/** Settings (PLAN §7): everything configurable or diagnosable, one card per concern. */
export function SettingsScreen(): ReactNode {
  const settings = toRemote(useSettings());
  return (
    <>
      <PageHeader
        title="Settings"
        summary="Config, agents, daemon, currency, theme and parse issues."
      />
      <RemoteView remote={settings}>
        {(settings) => (
          <>
            <GeneralCard settings={settings} />
            <AgentsSection agents={settings.agents} />
            <DaemonCard settings={settings} />
            <DisplayCard settings={settings} />
            <PreferencesCard settings={settings} />
            <ProtectedFoldersSection folders={settings.protected_folders} />
            <BackupsSection backups={settings.backups} />
            <ParseIssuesSection />
            <RecentAlertsSection />
            <EraseSection />
          </>
        )}
      </RemoteView>
    </>
  );
}
