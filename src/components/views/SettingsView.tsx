import { useEffect, useState } from "react";
import {
  buildRetentionPolicy,
  DEFAULT_KEEP_DAYS,
  RETENTION_OPTIONS,
  setRetentionPolicy,
  useSettings,
  type RetentionKind,
} from "../../useSettings";
import { Select } from "../ui/Select";
import { TextField } from "../ui/TextField";

/**
 * The Settings view — Privacy only for now. Shows whether recording consent
 * has been acknowledged and lets the user change the raw-transcript retention
 * policy, which persists (and prunes) immediately on change.
 */
export function SettingsView() {
  const { settings, error, setSettings } = useSettings();
  const [days, setDays] = useState(DEFAULT_KEEP_DAYS);
  const [saveError, setSaveError] = useState<string | null>(null);

  // Seed the day field from the loaded policy so editing it starts from the
  // stored value rather than the placeholder default.
  useEffect(() => {
    if (settings?.retention.policy === "keep_days") {
      setDays(settings.retention.days);
    }
  }, [settings]);

  const kind: RetentionKind = settings?.retention.policy ?? "keep_all";

  const apply = async (nextKind: RetentionKind, nextDays: number) => {
    setSaveError(null);
    try {
      const updated = await setRetentionPolicy(buildRetentionPolicy(nextKind, nextDays));
      setSettings(updated);
    } catch (err) {
      setSaveError(String(err));
    }
  };

  return (
    <section className="flex min-h-full flex-col p-xl">
      <div className="mx-auto flex w-full max-w-content flex-col gap-lg">
        <header className="flex flex-col gap-3xs">
          <p className="text-eyebrow uppercase tracking-wide text-text-faint">Settings</p>
          <h2 className="font-serif text-h2 text-text">Privacy</h2>
        </header>

        {error && <p className="text-body text-text-soft">Couldn't load settings: {error}</p>}

        {settings && (
          <div className="flex max-w-measure flex-col gap-lg">
            <p className="text-body text-text-soft">
              {settings.consent_acknowledged
                ? "Recording consent acknowledged."
                : "The consent nudge is shown before your first capture."}
            </p>

            <div className="flex flex-col gap-sm">
              <Select
                label="Retention"
                value={kind}
                onChange={(value) => apply(value as RetentionKind, days)}
                options={RETENTION_OPTIONS}
              />
              {kind === "keep_days" && (
                <TextField
                  label="Days to keep"
                  type="number"
                  min={1}
                  value={days}
                  onChange={(event) => setDays(Number(event.target.value))}
                  onBlur={() => apply("keep_days", days)}
                />
              )}
              <p className="text-cap text-text-faint">
                Discard after distilling applies to captures from now on — sessions
                distilled before you chose it are not removed.
              </p>
              {saveError && (
                <p role="alert" className="text-cap text-text-soft">
                  Couldn't save: {saveError}
                </p>
              )}
            </div>
          </div>
        )}
      </div>
    </section>
  );
}
