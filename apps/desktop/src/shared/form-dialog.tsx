import type { ReactNode } from "react";
import {
  Dialog,
  FieldError,
  Form,
  Heading,
  Input,
  Label,
  Modal,
  ModalOverlay,
  Text,
  TextField,
  type TextFieldProps,
} from "react-aria-components";
import { Button } from "./button";

interface FormDialogProps {
  readonly title: string;
  readonly isOpen: boolean;
  readonly onOpenChange: (open: boolean) => void;
  /** Label of the submit button, a verb: "Add rule". */
  readonly submitLabel: string;
  readonly isPending: boolean;
  /** Called once the fields pass their native validation. */
  readonly onSubmit: () => void;
  readonly children: ReactNode;
}

/**
 * A modal form: title, fields, Cancel and a primary submit. React Aria validates the fields on
 * submit (required, pattern, type) and shows each error under its field instead of the browser's
 * bubble, so `onSubmit` only runs on valid input.
 */
export function FormDialog(props: FormDialogProps): ReactNode {
  return (
    <ModalOverlay
      isOpen={props.isOpen}
      onOpenChange={props.onOpenChange}
      isDismissable={!props.isPending}
      className="fixed inset-0 z-30 flex items-start justify-center overflow-y-auto bg-scrim p-4 pt-[12vh]"
    >
      <Modal className="w-full max-w-md rounded-md border border-border bg-surface-raised shadow-lg outline-hidden">
        <Dialog className="flex flex-col gap-4 p-4 outline-hidden">
          {({ close }) => (
            <>
              <Heading slot="title" className="text-base font-semibold">
                {props.title}
              </Heading>
              <Form
                className="flex flex-col gap-4"
                onSubmit={(event) => {
                  event.preventDefault();
                  props.onSubmit();
                }}
              >
                {props.children}
                <div className="flex flex-wrap justify-end gap-2">
                  <Button variant="quiet" onPress={close} isDisabled={props.isPending}>
                    Cancel
                  </Button>
                  <Button type="submit" variant="primary" isPending={props.isPending}>
                    {props.submitLabel}
                  </Button>
                </div>
              </Form>
            </>
          )}
        </Dialog>
      </Modal>
    </ModalOverlay>
  );
}

/** A labelled text field with an optional hint and its validation error. */
export function FormTextField({
  label,
  description,
  ...field
}: Pick<
  TextFieldProps,
  "name" | "value" | "onChange" | "isRequired" | "autoFocus" | "inputMode" | "pattern"
> & {
  readonly label: string;
  readonly description?: string;
}): ReactNode {
  return (
    <TextField {...field} className="flex flex-col gap-1">
      <Label className="text-sm font-medium">{label}</Label>
      <Input className="min-h-9 rounded-md border border-border bg-surface px-2 text-sm data-invalid:border-danger" />
      {description === undefined ? null : (
        <Text slot="description" className="text-xs text-fg-muted">
          {description}
        </Text>
      )}
      <FieldError className="text-xs text-danger" />
    </TextField>
  );
}
