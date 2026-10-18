import { Settings } from "lucide-react";
import { type ReactNode, useState } from "react";
import { Tab, TabList, TabPanel, Tabs } from "react-aria-components";
import { Button } from "../shared/button";
import type { Currency, Fx } from "../shared/format";
import {
  ALL_TAB_ID,
  type Health,
  PERIODS,
  type Period,
  type RecapModel,
} from "../shared/recap-model";
import { SegmentedControl } from "../shared/segmented-control";
import { StateView } from "../shared/state-view";
import { RecapBlocks } from "./blocks";

const PERIOD_LABELS: Record<Period, string> = { today: "Today", week: "Week", month: "Month" };

const HEALTH: Record<Health, { readonly label: string; readonly dot: string }> = {
  ok: { label: "Healthy", dot: "bg-success" },
  degraded: { label: "Degraded", dot: "bg-warning" },
  offline: { label: "Offline", dot: "bg-danger" },
};

export interface RecapProps {
  readonly model: RecapModel;
  readonly currency: Currency;
  readonly fx: Fx;
  readonly timeZone: string;
  readonly onPeriodChange: (period: Period) => void;
  /** `null` on the `All` tab. */
  readonly onOpenDashboard: (agent: string | null) => void;
  readonly onOpenSettings: () => void;
  readonly onScanNow: () => void;
}

/** Menu-bar recap (360px wide): agent tabs, period switch, blocks, footer. */
export function Recap(props: RecapProps): ReactNode {
  const { model } = props;
  const [tabId, setTabId] = useState<string | null>(null);
  const selected = model.tabs.find((tab) => tab.id === tabId) ?? model.tabs[0];
  const health = HEALTH[model.health];
  const ctx = {
    nowMs: model.nowMs,
    asOf: model.asOf,
    currency: props.currency,
    fx: props.fx,
    timeZone: props.timeZone,
  };

  return (
    <div className="flex h-dvh w-full min-w-[320px] max-w-[360px] flex-col bg-surface text-fg">
      <div className="flex min-h-0 flex-1 flex-col gap-3 overflow-y-auto p-3">
        {selected === undefined ? (
          <StateView
            state={{
              kind: "empty",
              title: "No agents found",
              hint: "Start a Claude Code or pi session and usage will appear here.",
            }}
          />
        ) : (
          <Tabs
            selectedKey={selected.id}
            onSelectionChange={(key) => setTabId(String(key))}
            className="flex flex-col gap-3"
          >
            <TabList aria-label="Agents" className="flex gap-1 border-b border-border">
              {model.tabs.map((tab) => (
                <Tab
                  key={tab.id}
                  id={tab.id}
                  className="-mb-px min-h-9 cursor-pointer border-b-2 border-transparent px-3 text-sm font-medium text-fg-muted data-hovered:text-fg data-selected:border-accent data-selected:text-fg"
                >
                  {tab.label}
                </Tab>
              ))}
            </TabList>
            <SegmentedControl
              label="Period"
              options={PERIODS.map((value) => ({ value, label: PERIOD_LABELS[value] }))}
              value={model.period}
              onChange={props.onPeriodChange}
            />
            {model.tabs.map((tab) => (
              <TabPanel key={tab.id} id={tab.id} className="outline-offset-4">
                <RecapBlocks view={tab.view} ctx={ctx} />
              </TabPanel>
            ))}
          </Tabs>
        )}
      </div>
      <footer className="flex items-center gap-1 border-t border-border bg-surface-raised p-2">
        <span role="status" className="flex items-center gap-1.5 pr-1 text-xs text-fg-muted">
          <span className={`size-2 rounded-full ${health.dot}`} aria-hidden />
          {health.label}
        </span>
        <span className="flex-1" />
        <Button
          onPress={() =>
            props.onOpenDashboard(
              selected === undefined || selected.id === ALL_TAB_ID ? null : selected.id,
            )
          }
        >
          Open dashboard
        </Button>
        <Button variant="quiet" onPress={props.onScanNow}>
          Scan now
        </Button>
        <Button variant="quiet" aria-label="Settings" onPress={props.onOpenSettings}>
          <Settings className="size-4" aria-hidden />
        </Button>
      </footer>
    </div>
  );
}
