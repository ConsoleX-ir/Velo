import type { ReactNode } from "react";

type EmptyStateProps = {
  /** Hero mark image source; defaults to Velo's brand mark. */
  markSrc?: string;
  eyebrow?: string;
  /** Heading content; wrap emphasis in `<em>` for the red accent. */
  title: ReactNode;
  description: string;
  action?: ReactNode;
  footnote?: ReactNode;
};

/**
 * The hero empty state shown when the workspace has no downloads at all.
 * Structure (mark, eyebrow, heading, description, action, footnote) matches
 * the original inline markup; copy and actions are passed in by the caller so
 * the component stays free of app-level icons and routing.
 */
export function EmptyState({ markSrc = "/velo-mark.svg", eyebrow = "READY WHEN YOU ARE", title, description, action, footnote }: EmptyStateProps) {
  return (
    <div className="empty-state">
      <img className="empty-mark" src={markSrc} alt="" />
      <div className="empty-eyebrow"><span /> {eyebrow} <span /></div>
      <h2>{title}</h2>
      <p>{description}</p>
      {action}
      {footnote && <div className="empty-footnote">{footnote}</div>}
    </div>
  );
}

type FilteredEmptyProps = {
  icon: ReactNode;
  title: string;
  hint: string;
};

/**
 * The smaller centred variant used when a view or filter yields no rows.
 */
export function FilteredEmpty({ icon, title, hint }: FilteredEmptyProps) {
  return (
    <div className="filtered-empty">
      <div className="filtered-empty-icon">{icon}</div>
      <strong>{title}</strong>
      <span>{hint}</span>
    </div>
  );
}
