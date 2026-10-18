import {
  Cpu,
  Download,
  FileText,
  FolderKanban,
  Gauge,
  History,
  LayoutDashboard,
  Lightbulb,
  type LucideIcon,
  Receipt,
  Settings,
} from "lucide-react";

export interface NavItem {
  readonly to: string;
  readonly label: string;
  readonly icon: LucideIcon;
  /** One line on what the screen shows (PLAN §7). */
  readonly summary: string;
}

/** The ten screens, in sidebar order. Also the source of the route tree. */
export const NAV_ITEMS = [
  {
    to: "/",
    label: "Overview",
    icon: LayoutDashboard,
    summary: "Paid cost against API value, quota meters, daily cost, top models and projects.",
  },
  {
    to: "/quota",
    label: "Quota",
    icon: Gauge,
    summary: "Utilization of every quota window over time, with reset markers.",
  },
  {
    to: "/billing",
    label: "Billing",
    icon: Receipt,
    summary: "Subscriptions, monthly paid cost against API value, budgets and the plan advisor.",
  },
  {
    to: "/sessions",
    label: "Sessions",
    icon: History,
    summary: "Every session with tokens, cost and duration, and a drilldown per event.",
  },
  {
    to: "/insights",
    label: "Insights",
    icon: Lightbulb,
    summary: "Tool calls, API errors, compactions and subagent cost share.",
  },
  {
    to: "/models",
    label: "Models",
    icon: Cpu,
    summary: "Tokens and cost per model, cost sources and unpriced warnings.",
  },
  {
    to: "/projects",
    label: "Projects",
    icon: FolderKanban,
    summary: "Cost per project and client, with branch and worktree drilldowns.",
  },
  {
    to: "/reports",
    label: "Reports",
    icon: FileText,
    summary: "Pick a period, scope and format, preview it, then save.",
  },
  {
    to: "/export",
    label: "Export",
    icon: Download,
    summary: "Sink health, run history, and manual export.",
  },
  {
    to: "/settings",
    label: "Settings",
    icon: Settings,
    summary: "Config, agents, daemon, currency, theme and parse issues.",
  },
] as const satisfies readonly NavItem[];
