import { useT } from "../lib/i18n/I18n";
import { say } from "../lib/i18n/text";
import type { EncoderReport, Msg } from "../lib/ipc";

interface Props {
  enabled: boolean;
  busy: boolean;
  report: EncoderReport | null;
  error: Msg | null;
  onRun: () => void;
}

const CODEC: Record<string, string> = { hevc: "H.265", av1: "AV1", h264: "H.264" };

export function EncoderPanel({ enabled, busy, report, error, onRun }: Props) {
  const t = useT();
  const w = t.diag.encoders;
  return (
    <section className="panel" aria-labelledby="enc-title">
      <div className="panel-head">
        <h2 id="enc-title">{w.title}</h2>
        <button className="btn" onClick={onRun} disabled={!enabled || busy}>
          {busy ? w.running : report ? t.setup.checkAgain : w.start}
        </button>
      </div>
      <p className="muted small">{w.sub}</p>
      {error && <p className="error">{say(t, error)}</p>}
      {report && (
        <>
          <p>
            {w.preferred} — H.265: <strong className="mono">{report.preferredHevc ?? w.none}</strong> · AV1:{" "}
            <strong className="mono">{report.preferredAv1 ?? w.none}</strong>
          </p>
          <div className="table-wrap">
            <table className="data">
              <thead>
                <tr>
                  <th scope="col">{w.col.encoder}</th>
                  <th scope="col">{w.col.codec}</th>
                  <th scope="col">{w.col.kind}</th>
                  <th scope="col">{w.col.result}</th>
                  <th scope="col" className="num">
                    {w.col.time}
                  </th>
                  <th scope="col">{w.col.error}</th>
                </tr>
              </thead>
              <tbody>
                {report.tests.map((r) => (
                  <tr key={r.name}>
                    <th scope="row" className="mono">
                      {r.name}
                    </th>
                    <td>{CODEC[r.codec] ?? r.codec}</td>
                    <td>{r.hardware ? w.hardware : "CPU"}</td>
                    <td className={r.ok ? "ok-text" : "warn-text"}>{r.ok ? w.ok : w.failed}</td>
                    <td className="num mono">{r.millis}ms</td>
                    <td className="small muted">{r.error ? say(t, r.error) : ""}</td>
                  </tr>
                ))}
              </tbody>
            </table>
          </div>
          {report.tests.length === 0 && <p className="muted">{w.noneToTest}</p>}
        </>
      )}
    </section>
  );
}
