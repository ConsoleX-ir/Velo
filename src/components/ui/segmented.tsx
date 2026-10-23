import { useRef, type KeyboardEvent as ReactKeyboardEvent, type ReactNode } from "react";
import { cn } from "../../lib/utils";

export type SegmentedOption<T extends string> = {
  value: T;
  label: string;
  icon?: ReactNode;
};

type SegmentedControlProps<T extends string> = {
  value: T;
  options: SegmentedOption<T>[];
  onChange: (value: T) => void;
  "aria-label": string;
  /** Container class, e.g. the boxed `.segmented-control` look or a plain flex row. */
  className?: string;
  /** Class applied to every option button, e.g. `.filter-chip` for the chips-like row. */
  optionClassName?: string;
};

/**
 * A WAI-ARIA radiogroup styled as a segmented control (composition and
 * roving-tabindex semantics adapted from shadcn/ui's Tabs/RadioGroup
 * patterns, kept dependency-free). Arrow keys move selection with wrap-around,
 * Home/End jump to the ends, and the selected option keeps `tabIndex={0}` so
 * Tab lands on the group once. Selection follows focus, matching native
 * radio-button behaviour.
 */
export function SegmentedControl<T extends string>({ value, options, onChange, className = "", optionClassName = "", ...groupProps }: SegmentedControlProps<T>) {
  const optionRefs = useRef(new Map<T, HTMLButtonElement>());

  function selectAndFocus(next: T) {
    onChange(next);
    optionRefs.current.get(next)?.focus();
  }

  function handleKeyDown(event: ReactKeyboardEvent<HTMLButtonElement>, current: T) {
    const currentIndex = options.findIndex((option) => option.value === current);
    if (currentIndex < 0 || options.length === 0) return;
    if (event.key === "Home") {
      event.preventDefault();
      selectAndFocus(options[0].value);
      return;
    }
    if (event.key === "End") {
      event.preventDefault();
      selectAndFocus(options[options.length - 1].value);
      return;
    }
    const steps: Record<string, number> = { ArrowLeft: -1, ArrowUp: -1, ArrowRight: 1, ArrowDown: 1 };
    const step = steps[event.key];
    if (step === undefined) return;
    event.preventDefault();
    selectAndFocus(options[(currentIndex + step + options.length) % options.length].value);
  }

  return (
    <div role="radiogroup" className={className} {...groupProps}>
      {options.map((option) => {
        const selected = option.value === value;
        return (
          <button
            key={option.value}
            ref={(element) => {
              if (element) optionRefs.current.set(option.value, element);
              else optionRefs.current.delete(option.value);
            }}
            type="button"
            role="radio"
            aria-checked={selected}
            tabIndex={selected ? 0 : -1}
            className={cn(optionClassName, selected && "selected")}
            onClick={() => onChange(option.value)}
            onKeyDown={(event) => handleKeyDown(event, option.value)}
          >
            {option.icon}
            {option.label}
          </button>
        );
      })}
    </div>
  );
}
