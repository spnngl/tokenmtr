import { type ReactNode, useState } from "react";
import { Form } from "react-aria-components";
import { useDataAction } from "../../shared/actions";
import { Button } from "../../shared/button";
import { FormTextField } from "../../shared/form-dialog";
import type { Settings } from "../../shared/schemas";
import { Card } from "../../shared/ui";

interface GitDefaults {
  readonly remote: string;
  readonly branch: string | null;
}

/** `[git]`: the remote whose URL keys projects, and the work branch kept for branch statistics. */
export function GitSection({ settings }: { readonly settings: Settings }): ReactNode {
  const [remote, setRemote] = useState(settings.git.default_remote);
  const [branch, setBranch] = useState(settings.git.default_branch ?? "");
  const save = useDataAction(
    (ds, defaults: GitDefaults) => ds.setGitDefaults(defaults.remote, defaults.branch),
    {
      failure: "Could not change the git defaults",
      success: (_, defaults) => `Projects are now keyed by the ${defaults.remote} remote`,
    },
  );
  return (
    <Card title="Git">
      <p className="text-sm text-fg-muted">
        A repository's project is named after the URL of this remote, else of origin, else of its
        first remote. Projects move to the new key on the next scan.
      </p>
      <Form
        className="flex flex-col gap-3"
        onSubmit={(event) => {
          event.preventDefault();
          const trimmed = branch.trim();
          save.mutate({ remote: remote.trim(), branch: trimmed === "" ? null : trimmed });
        }}
      >
        <FormTextField
          label="Default remote"
          name="default_remote"
          value={remote}
          onChange={setRemote}
          isRequired
          pattern="\s*\S+\s*"
          description="The remote whose URL keys projects, such as origin or upstream."
        />
        <FormTextField
          label="Default branch"
          name="default_branch"
          value={branch}
          onChange={setBranch}
          pattern="\s*\S*\s*"
          description="Your work branch, such as main. Kept for upcoming branch statistics."
        />
        <div>
          <Button type="submit" isPending={save.isPending}>
            Save
          </Button>
        </div>
      </Form>
    </Card>
  );
}
