import axe from "axe-core";

/**
 * Test helper: ids of axe violations inside `container`. jsdom has no layout or painting, so the
 * colour-contrast rule is off here; contrast is checked on the tokens instead
 * (`contrast.ts` of the ui-expert skill, run against `styles.css`).
 */
export async function axeViolations(container: Element): Promise<string[]> {
  const results = await axe.run(container, { rules: { "color-contrast": { enabled: false } } });
  return results.violations.map((violation) => violation.id);
}
