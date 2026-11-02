import { Dialog } from "./ui/dialog";
import { Button } from "./ui/button";
import { Icon } from "./icons";

type ConfirmDialogProps = {
  title: string;
  body: string;
  /** The record the action applies to, echoed back so intent is unambiguous. */
  target?: string;
  confirmLabel: string;
  busy?: boolean;
  onConfirm: () => void;
  onClose: () => void;
};

/**
 * Small destructive-action guard. Velo keeps destructive flows one click
 * away everywhere else; this panel exists only where a list record would be
 * discarded, and it states plainly that the file on disk is untouched.
 */
export function ConfirmDialog({ title, body, target, confirmLabel, busy = false, onConfirm, onClose }: ConfirmDialogProps) {
  return (
    <Dialog
      open
      onClose={busy ? () => undefined : onClose}
      closeOnEscape={!busy}
      closeOnOutsideClick={!busy}
      labelledBy="confirm-title"
      panelClassName="confirm-dialog"
    >
      <div className="confirm-icon" aria-hidden="true">
        <Icon name="alert" size={19} />
      </div>
      <h2 id="confirm-title">{title}</h2>
      <p className="confirm-body">{body}</p>
      {target && <span className="confirm-target" title={target}>{target}</span>}
      <div className="dialog-actions">
        <Button variant="secondary" onClick={onClose} disabled={busy}>Cancel</Button>
        <Button variant="primary" className="danger-button" onClick={onConfirm} loading={busy}>{confirmLabel}</Button>
      </div>
    </Dialog>
  );
}
