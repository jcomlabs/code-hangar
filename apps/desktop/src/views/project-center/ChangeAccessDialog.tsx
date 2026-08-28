import { LockKeyhole, ShieldCheck, X } from "lucide-react";
import { memo, useEffect, useState } from "react";

export const ChangeAccessDialog = memo(function ChangeAccessDialog({
  projectName,
  onCancel,
  onUnlock
}: {
  projectName: string;
  onCancel: () => void;
  onUnlock: () => void;
}) {
  const [acknowledged, setAcknowledged] = useState(false);
  const [typedName, setTypedName] = useState("");

  useEffect(() => {
    setAcknowledged(false);
    setTypedName("");
  }, [projectName]);

  const matches = typedName.trim() === projectName.trim();
  return (
    <div className="modal-overlay change-access-overlay" role="dialog" aria-modal="true" aria-label="Unlock project changes">
      <div className="modal change-access-dialog">
        <header className="change-review-heading">
          <div>
            <span>Project files are locked</span>
            <strong><LockKeyhole size={18} /> Unlock changes for this project</strong>
            <small>{projectName}</small>
          </div>
          <button className="icon-button" type="button" aria-label="Keep project changes locked" onClick={onCancel}><X size={16} /></button>
        </header>

        <div className="change-access-warning">
          <ShieldCheck size={20} />
          <div>
            <strong>Opening and reviewing projects never needs this permission.</strong>
            <p>Unlock only when you deliberately want Code Hangar to change one local file. Unlocking does not write anything by itself.</p>
          </div>
        </div>

        <ul className="change-access-rules">
          <li>Every apply still shows the exact removed and added lines.</li>
          <li>A verified previous version is created before the write.</li>
          <li>Code Hangar never commits, pushes or changes a Git branch.</li>
          <li>The lock returns when you change project or reopen the app.</li>
        </ul>

        <label className="change-access-check">
          <input type="checkbox" checked={acknowledged} onChange={(event) => setAcknowledged(event.target.checked)} />
          <span>I understand that an applied change edits a real file on this computer.</span>
        </label>
        <label className="change-access-name">
          Type <strong>{projectName}</strong> to unlock
          <input value={typedName} onChange={(event) => setTypedName(event.target.value)} autoComplete="off" spellCheck={false} />
        </label>

        <footer className="change-review-actions">
          <span>Nothing changes until a separate reviewed Apply.</span>
          <div>
            <button type="button" onClick={onCancel}>Keep locked</button>
            <button type="button" className="primary-button" onClick={onUnlock} disabled={!acknowledged || !matches}>
              <LockKeyhole size={15} /> Unlock for this project
            </button>
          </div>
        </footer>
      </div>
    </div>
  );
});
