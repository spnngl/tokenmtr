import type { ReactNode } from "react";
import { Switch } from "react-aria-components";
import { useDataAction } from "../../shared/actions";
import { MENUBAR_RINGS, type MenubarRing, type Settings } from "../../shared/schemas";
import { Card } from "../../shared/ui";

const RING_TEXT: Record<MenubarRing, string> = {
  session: "Session (5-hour window)",
  weekly: "Weekly (7-day window)",
  per_model: "Per model (each model's weekly window)",
};

interface SettingSwitchProps {
  readonly isSelected: boolean;
  readonly onChange: (selected: boolean) => void;
  readonly children: ReactNode;
}

/** A labelled on/off switch. */
export function SettingSwitch({ isSelected, onChange, children }: SettingSwitchProps): ReactNode {
  return (
    <Switch
      isSelected={isSelected}
      onChange={onChange}
      className="group flex min-h-9 w-fit items-center gap-2 text-sm"
    >
      <span className="flex h-5 w-9 shrink-0 items-center rounded-full border border-border bg-surface-muted px-0.5 transition-colors group-data-selected:bg-accent group-data-focus-visible:outline-2 group-data-focus-visible:outline-offset-2 group-data-focus-visible:outline-ring">
        <span className="size-4 rounded-full bg-fg-muted transition-transform group-data-selected:translate-x-4 group-data-selected:bg-accent-fg" />
      </span>
      {children}
    </Switch>
  );
}

interface RingChoice {
  readonly rings: readonly MenubarRing[];
  readonly hidden: readonly string[];
}

/** Which quota windows the menu bar draws as rings, and for which profiles (`[menubar]`). */
export function MenubarSection({ settings }: { readonly settings: Settings }): ReactNode {
  const save = useDataAction(
    (ds, choice: RingChoice) => ds.setMenubarRings(choice.rings, choice.hidden),
    { failure: "Could not change the menu-bar rings" },
  );
  // Shows the requested choice while it is applied; a failure toasts and falls back.
  const current: RingChoice = save.isPending
    ? save.variables
    : { rings: settings.tray.rings, hidden: settings.tray.hidden_profiles };
  const toggleRing = (ring: MenubarRing, on: boolean): void => {
    const rings = MENUBAR_RINGS.filter((r) => (r === ring ? on : current.rings.includes(r)));
    save.mutate({ rings, hidden: current.hidden });
  };
  const toggleProfile = (profile: string, shown: boolean): void => {
    const hidden = shown
      ? current.hidden.filter((p) => p !== profile)
      : [...current.hidden, profile];
    save.mutate({ rings: current.rings, hidden });
  };
  const profiles = [
    ...new Set(
      settings.agents
        .filter((agent) => agent.followed && agent.capabilities.quota)
        .flatMap((agent) => agent.profiles.map((p) => p.profile)),
    ),
  ];
  return (
    <Card title="Menu bar">
      <p className="text-sm text-fg-muted">
        One ring per quota window and profile shows how much is used: amber from 70%, red from 90%,
        dashed when the quota cannot be read. With no ring, the menu bar shows the plain icon.
      </p>
      <fieldset className="flex flex-col">
        <legend className="text-sm font-medium">Rings</legend>
        {MENUBAR_RINGS.map((ring) => (
          <SettingSwitch
            key={ring}
            isSelected={current.rings.includes(ring)}
            onChange={(on) => toggleRing(ring, on)}
          >
            {RING_TEXT[ring]}
          </SettingSwitch>
        ))}
      </fieldset>
      {profiles.length > 1 ? (
        <fieldset className="flex flex-col">
          <legend className="text-sm font-medium">Profiles</legend>
          {profiles.map((profile) => (
            <SettingSwitch
              key={profile}
              isSelected={!current.hidden.includes(profile)}
              onChange={(shown) => toggleProfile(profile, shown)}
            >
              {profile}
            </SettingSwitch>
          ))}
        </fieldset>
      ) : null}
    </Card>
  );
}
