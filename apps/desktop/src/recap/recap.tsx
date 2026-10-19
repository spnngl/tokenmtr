import { Settings } from "lucide-react";
import { type ReactNode, useLayoutEffect, useRef, useState } from "react";
import { Tab, TabList, TabPanel, Tabs } from "react-aria-components";
import { Button } from "../shared/button";
import { PERIODS, type Period, type RecapSnapshot } from "../shared/schemas";
import { SegmentedControl } from "../shared/segmented-control";
import { StateView } from "../shared/state-view";
import { RecapBlocks } from "./blocks";
import { summarizeHealth } from "./health";

const PERIOD_LABELS: Record<Period, string> = { today: "Today", week: "Week", month: "Month" };

const DOT: Record<"ok" | "warning" | "error", string> = {
  ok: "bg-success",
  warning: "bg-warning",
  error: "bg-danger",
};

const ALL_TAB_ID = "all";

export interface RecapProps {
  readonly snapshot: RecapSnapshot;
  readonly period: Period;
  readonly timeZone: string;
  readonly now: number;
  readonly onPeriodChange: (period: Period) => void;
  /** `null` on the `All` tab. */
  readonly onOpenDashboard: (agent: string | null) => void;
  readonly onOpenSettings: () => void;
  readonly onScanNow: () => void;
  /** A scan started by `onScanNow` is running. */
  readonly isScanning: boolean;
  /** Display name per agent id (`settings.agents`); an agent missing from it shows its id. */
  readonly agentLabels: ReadonlyMap<string, string>;
  /** Called with the height (CSS pixels) that shows the whole recap, each time it changes. */
  readonly onContentHeight?: (height: number) => void;
}

/**
 * Reports the summed heights of `parts` whenever one of them resizes, so the popover window can
 * grow or shrink to show everything without scrolling.
 */
function useContentHeight(
  parts: readonly { readonly current: HTMLElement | null }[],
  onHeight: ((height: number) => void) | undefined,
): void {
  const reported = useRef<number | null>(null);
  const callback = useRef(onHeight);
  callback.current = onHeight;
  // biome-ignore lint/correctness/useExhaustiveDependencies: the refs are stable; observe once.
  useLayoutEffect(() => {
    const elements = parts.flatMap((part) => (part.current === null ? [] : [part.current]));
    if (elements.length === 0) {
      return;
    }
    const report = (): void => {
      const height = Math.ceil(elements.reduce((sum, element) => sum + element.offsetHeight, 0));
      if (height !== reported.current) {
        reported.current = height;
        callback.current?.(height);
      }
    };
    const observer = new ResizeObserver(report);
    for (const element of elements) {
      observer.observe(element);
    }
    report();
    return () => observer.disconnect();
  }, []);
}

/**
 * Menu-bar recap (360px wide): a header with the actions and the health line, then agent tabs,
 * period switch and blocks. It reports its natural height so the popover shows it whole.
 */
export function Recap(props: RecapProps): ReactNode {
  const { snapshot } = props;
  const header = useRef<HTMLElement>(null);
  const content = useRef<HTMLDivElement>(null);
  useContentHeight([header, content], props.onContentHeight);
  const tabs = snapshot.tabs.map((recap) => ({
    id: recap.agent ?? ALL_TAB_ID,
    label: recap.agent === null ? "All" : (props.agentLabels.get(recap.agent) ?? recap.agent),
    recap,
  }));
  const [tabId, setTabId] = useState<string | null>(null);
  const selected = tabs.find((tab) => tab.id === tabId) ?? tabs[0];
  const health = summarizeHealth(snapshot.system);
  const ctx = { nowMs: props.now, timeZone: props.timeZone };

  return (
    <div className="flex h-dvh w-full min-w-[320px] max-w-[360px] flex-col bg-surface text-fg">
      <header
        ref={header}
        className="flex flex-col gap-1.5 border-b border-border bg-surface-raised p-2"
      >
        <div className="flex items-center gap-1">
          <Button
            onPress={() =>
              props.onOpenDashboard(
                selected === undefined || selected.id === ALL_TAB_ID ? null : selected.id,
              )
            }
          >
            Open dashboard
          </Button>
          <span className="flex-1" />
          <Button variant="quiet" isPending={props.isScanning} onPress={props.onScanNow}>
            Scan now
          </Button>
          <Button variant="quiet" aria-label="Settings" onPress={props.onOpenSettings}>
            <Settings className="size-4" aria-hidden />
          </Button>
        </div>
        <span role="status" className="flex items-start gap-1.5 px-1 text-xs text-fg-muted">
          <span className={`mt-1 size-2 shrink-0 rounded-full ${DOT[health.level]}`} aria-hidden />
          <span>{health.label}</span>
        </span>
      </header>
      <div className="min-h-0 flex-1 overflow-y-auto">
        <div ref={content} className="flex flex-col gap-3 p-3">
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
              <TabList
                aria-label="Agents"
                className="flex gap-1 overflow-x-auto border-b border-border"
              >
                {tabs.map((tab) => (
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
                value={props.period}
                onChange={props.onPeriodChange}
              />
              {tabs.map((tab) => (
                <TabPanel key={tab.id} id={tab.id} className="outline-offset-4">
                  <RecapBlocks recap={tab.recap} ctx={ctx} />
                </TabPanel>
              ))}
            </Tabs>
          )}
        </div>
      </div>
    </div>
  );
}
