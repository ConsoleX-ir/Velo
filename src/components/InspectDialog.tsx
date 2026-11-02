import { Dialog } from "./ui/dialog";
import { StatePill } from "./ui/badge";
import { Icon } from "./icons";
import {
  destinationDirectory,
  fileTypeInfo,
  formatBytes,
  formatEta,
  formatRate,
  progressPercent,
  type DownloadDto,
} from "../model";

type InspectDialogProps = {
  item: DownloadDto;
  onClose: () => void;
};

/**
 * Inspect — the technical readout for one transfer, rendered exclusively
 * from the fields the engine already publishes on the DTO. Structured as a
 * two-column spec sheet: machine keys on the left, values on the right, so
 * dense metadata stays scannable instead of becoming an info dump.
 */
export function InspectDialog({ item, onClose }: InspectDialogProps) {
  const percent = progressPercent(item);
  const kind = fileTypeInfo(item.fileName);
  const rate = formatRate(item.speedBytesPerSecond);
  const eta = formatEta(item.etaSeconds);

  const sizeValue = item.totalBytes !== null
    ? `${formatBytes(item.bytesWritten)} of ${formatBytes(item.totalBytes)}${percent !== null ? ` · ${Math.floor(percent)}%` : ""}`
    : `${formatBytes(item.bytesWritten)} · total size unknown`;

  return (
    <Dialog open onClose={onClose} labelledBy="inspect-title" panelClassName="inspect-dialog">
      <div className="dialog-top">
        <span className="micro-label">Inspect</span>
        <button className="icon-button dialog-close" onClick={onClose} aria-label="Close inspect panel">
          <Icon name="close" />
        </button>
      </div>

      <div className="inspect-head">
        <div className={`row-icon tone-${kind.tone}`} aria-hidden="true">
          {kind.tone === "generic" ? <Icon name="file" size={18} /> : <span className="row-icon-code">{kind.code}</span>}
        </div>
        <div className="inspect-head-copy">
          <strong id="inspect-title" title={item.fileName}>{item.fileName}</strong>
          <span><StatePill state={item.state} /></span>
        </div>
      </div>

      <div className="inspect-section">
        <div className="form-section-label micro-label">Transfer</div>
        <div className="inspect-grid">
          <span className="inspect-key">Destination</span>
          <span className="inspect-val">
            <span className="path-text" title={item.destination}>{item.destination}</span>
          </span>
          <span className="inspect-key">Folder</span>
          <span className="inspect-val">
            <span className="path-text" title={destinationDirectory(item.destination)}>{destinationDirectory(item.destination)}</span>
          </span>
          <span className="inspect-key">Data</span>
          <span className="inspect-val">{sizeValue}</span>
          <span className="inspect-key">Speed</span>
          <span className="inspect-val">{item.state === "downloading" ? rate ?? "Measuring…" : "—"}</span>
          <span className="inspect-key">Remaining</span>
          <span className="inspect-val">{item.state === "downloading" ? eta ?? "Calculating…" : "—"}</span>
          {item.scheduledAtUnixMs != null && (
            <>
              <span className="inspect-key">Scheduled</span>
              <span className="inspect-val">{new Date(item.scheduledAtUnixMs).toLocaleString()}</span>
            </>
          )}
        </div>
      </div>

      <div className="inspect-section">
        <div className="form-section-label micro-label">Record</div>
        <div className="inspect-grid">
          <span className="inspect-key">Job id</span>
          <span className="inspect-val"><span className="mono">{item.id}</span></span>
          <span className="inspect-key">Generation</span>
          <span className="inspect-val">{item.generation}</span>
          <span className="inspect-key">Revision</span>
          <span className="inspect-val">{item.revision}</span>
          <span className="inspect-key">Queue order</span>
          <span className="inspect-val">{item.order}</span>
        </div>
      </div>

      {item.error && (
        <div className="inspect-error" role="alert">
          <Icon name="alert" size={15} />
          <span>{item.error}</span>
        </div>
      )}
    </Dialog>
  );
}
