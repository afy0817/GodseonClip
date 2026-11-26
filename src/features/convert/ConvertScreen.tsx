// 일괄 변환 (canvas UI mockup Convert.dc.html): files and folders in, every listed file converted to
// H.265 or AV1 with one set of settings, each as its own 작업 큐 item. Rust lists folders, reads the
// files, plans and names every conversion (convert.rs); this screen only says what to do.

import { open } from "@tauri-apps/plugin-dialog";
import { useEffect, useMemo, useState } from "react";
import { useT } from "../../lib/i18n/I18n";
import { say } from "../../lib/i18n/text";
import { api, type ConvertQueued, type FfmpegInfo, type Msg, type QueueItem, toMsg } from "../../lib/ipc";
import { formatBytes } from "../../lib/time";
import { type QueueState } from "../../lib/useQueue";
import { CheckIcon, FolderIcon, UploadIcon, WarningIcon } from "../editor/icons";
import { qualities } from "../editor/exportOptions";
import type { FfmpegState } from "../editor/Stage";
import { TopBar, type WorkKind } from "../editor/TopBar";
import { sourceLabel } from "../setup/setupView";
import { useFileDrop } from "../editor/useFileDrop";
import { activeCount } from "../queue/queueView";
import {
  audioChoices,
  codecCell,
  codecs,
  type ConvertChoice,
  CONVERT_EXTENSIONS,
  containersFor,
  DEFAULT_CONVERT,
  defaultSuffix,
  encoderInUse,
  encoderLabel,
  encoderNote,
  encodersFor,
  estimateCell,
  estimateLine,
  estimateSettingsKey,
  estimateTotals,
  lengthCell,
  listHeading,
  listTotals,
  nameOf,
  noticeText,
  outputPattern,
  qualityNote,
  resolutions,
  rowStatus,
  sizeCell,
  suffixOf,
  suffixProblem,
  toSettings,
  totalsLine,
  withCodec,
} from "./convertView";
import { useConvertList } from "./useConvertList";
import { useEncoderOptions } from "./useEncoderOptions";
import { useEstimates } from "./useEstimates";
import "../editor/editor.css";
import "./convert.css";

interface Props {
  /** False while another screen covers this one. */
  active: boolean;
  ffmpeg: FfmpegState & { info: FfmpegInfo | null };
  queue: QueueState;
  onKind: (k: WorkKind) => void;
  onOpenQueue: () => void;
  onOpenSettings: () => void;
}

const CONTAINER_LABELS = { mp4: "MP4", mkv: "MKV", mov: "MOV", webm: "WebM" } as const;

export function ConvertScreen({ active, ffmpeg, queue, onKind, onOpenQueue, onOpenSettings }: Props) {
  const t = useT();
  const w = t.convertScreen;
  const list = useConvertList();
  const [choice, setChoice] = useState<ConvertChoice>(DEFAULT_CONVERT);
  const [showAdvanced, setShowAdvanced] = useState(true);
  const [starting, setStarting] = useState(false);
  const [started, setStarted] = useState<ConvertQueued | null>(null);
  const [startError, setStartError] = useState<Msg | null>(null);
  const encoders = useEncoderOptions(active, ffmpeg.ready ? ffmpeg.path : null);
  const set = (patch: Partial<ConvertChoice>) => setChoice((c) => ({ ...c, ...patch }));

  // A picked encoder that a new test no longer lists goes back to "자동".
  useEffect(() => {
    if (choice.encoder && encoders.options && !encodersFor(encoders.options, choice.codec).some((e) => e.name === choice.encoder)) {
      set({ encoder: null });
    }
  }, [encoders.options, choice.encoder, choice.codec]);

  const items = useMemo(() => new Map<number, QueueItem>(queue.items.map((i) => [i.id, i])), [queue.items]);
  const statuses = list.rows.map((r) => rowStatus(t, r, choice, r.itemId === null ? undefined : items.get(r.itemId)));
  const totals = listTotals(list.rows, statuses);
  const heading = listHeading(t, list.rows.length, totals, choice.codec);
  const selectable = list.rows.filter((_, i) => statuses[i].selectable);
  const allChecked = selectable.length > 0 && selectable.every((r) => r.checked);
  // 예상 크기: the files going in at the next 변환 시작, tried one at a time with these settings.
  const included = list.rows.filter((_, i) => statuses[i].included).map((r) => r.path);
  const estimates = useEstimates(ffmpeg.ready, included, toSettings(choice), estimateSettingsKey(choice));
  const estimate = estimateLine(t, estimateTotals(list.rows, statuses, estimates.entries), estimates.paused);

  const canAdd = ffmpeg.ready && !list.adding;
  const dragged = useFileDrop(active && canAdd, (paths) => void list.add(paths));

  const webmWithoutOpus = !ffmpeg.info?.features.libopus;
  const suffixWhy = suffixProblem(t, suffixOf(choice));
  const missingCodec = (codec: "hevc" | "av1") => encoders.options !== null && encoders.options.auto[codec] === null;
  const inUse = encoderInUse(encoders.options, choice);
  const blocked: string | null = !ffmpeg.ready
    ? w.blockedNoFfmpeg
    : suffixWhy
      ? w.blockedSuffix(suffixWhy)
      : choice.container === "webm" && webmWithoutOpus
        ? w.blockedOpus
        : encoders.options && !inUse
          ? w.blockedEncoder
          : null;

  const addFiles = async () => {
    const picked = await open({ multiple: true, directory: false, title: w.addFilesTitle, filters: [{ name: t.files.videoFilter, extensions: CONVERT_EXTENSIONS }] });
    if (picked) await list.add(Array.isArray(picked) ? picked : [picked]);
  };
  const addFolders = async () => {
    const picked = await open({ multiple: true, directory: true, title: w.addFoldersTitle });
    if (picked) await list.add(Array.isArray(picked) ? picked : [picked]);
  };
  const pickOutputDir = async () => {
    const picked = await open({ multiple: false, directory: true, title: w.outputDirTitle, defaultPath: choice.outputDir ?? undefined });
    if (typeof picked === "string") set({ outputDir: picked });
  };

  const start = async () => {
    const paths = list.rows.filter((_, i) => statuses[i].included).map((r) => r.path);
    if (paths.length === 0) return;
    setStarting(true);
    setStartError(null);
    setStarted(null);
    try {
      const r = await api.queueConvert({ paths, settings: toSettings(choice) });
      list.markQueued(r.queued);
      setStarted(r);
    } catch (e) {
      setStartError(toMsg(e));
    } finally {
      setStarting(false);
    }
  };

  return (
    <div className="ed ed-split cv">
      <TopBar
        kind="convert"
        onKind={onKind}
        queueCount={activeCount(queue.items)}
        queueCurrent={false}
        onOpenQueue={onOpenQueue}
        onOpenSettings={onOpenSettings}
      />

      <div className="ed-body">
        <main className="ed-main cv-main">
          <div className={`cv-drop${dragged ? " over" : ""}${!ffmpeg.ready ? " blocked" : ""}`}>
            <div className="cv-drop-text">
              <span className="cv-drop-icon">
                <UploadIcon />
              </span>
              <div>
                <b>
                  {dragged ? w.dropOver(dragged.length === 1 ? nameOf(dragged[0]) : null, dragged.length) : w.dropTitle}
                </b>
                <span>{!ffmpeg.ready ? (ffmpeg.locating ? w.dropLocating : w.dropNeedsFfmpeg) : w.dropSub}</span>
              </div>
            </div>
            <div className="cv-drop-actions">
              <button type="button" className="ed-btn" disabled={!canAdd} onClick={() => void addFiles()}>
                {w.addFiles}
              </button>
              <button type="button" className="ed-btn" disabled={!canAdd} onClick={() => void addFolders()}>
                <FolderIcon />
                {w.addFolders}
              </button>
            </div>
          </div>

          {list.notices.length > 0 && (
            <div className="cv-notices" role="status">
              <ul>
                {list.notices.map((n, i) => (
                  <li key={i}>{noticeText(t, n)}</li>
                ))}
              </ul>
              <button type="button" className="link small" onClick={list.dismissNotices}>
                {w.close}
              </button>
            </div>
          )}

          <div className="cv-head">
            <h1>
              {heading.title}
              <span>{heading.sub}</span>
              {list.adding && <span> · {w.adding}</span>}
            </h1>
            <button type="button" className="link" disabled={list.rows.length === 0} onClick={list.clear}>
              {w.clear}
            </button>
          </div>

          <div className="cv-table-wrap">
            <table className="cv-table">
              <thead>
                <tr>
                  <th scope="col" className="cv-col-check">
                    <label className="cv-check">
                      <input
                        type="checkbox"
                        aria-label={w.selectAll}
                        checked={allChecked}
                        disabled={selectable.length === 0}
                        onChange={() => list.setChecked(selectable.map((r) => r.path), !allChecked)}
                      />
                    </label>
                  </th>
                  <th scope="col">{w.col.file}</th>
                  <th scope="col" className="cv-col-codec cv-wide">
                    {w.col.codec}
                  </th>
                  <th scope="col" className="cv-col-size cv-wide">
                    {w.col.size}
                  </th>
                  <th scope="col" className="cv-col-num cv-wide num">
                    {w.col.length}
                  </th>
                  <th scope="col" className="cv-col-num cv-wide num">
                    {w.col.bytes}
                  </th>
                  <th scope="col" className="cv-col-est num">
                    {w.col.estimate}
                  </th>
                  <th scope="col" className="cv-col-state">
                    {w.col.state}
                  </th>
                </tr>
              </thead>
              <tbody>
                {list.rows.length === 0 ? (
                  <tr>
                    <td colSpan={8} className="cv-empty">
                      {w.empty}
                    </td>
                  </tr>
                ) : (
                  list.rows.map((row, i) => {
                    const st = statuses[i];
                    const info = row.probe.state === "ready" ? row.probe.info : null;
                    const item = row.itemId === null ? undefined : items.get(row.itemId);
                    const est = estimateCell(t, st, estimates.entries.get(row.path), estimates.paused, item);
                    return (
                      <tr key={row.path} className={st.included ? "on" : undefined}>
                        <td>
                          <label className="cv-check">
                            <input
                              type="checkbox"
                              aria-label={w.convertRow(row.name)}
                              checked={st.included}
                              disabled={!st.selectable}
                              onChange={() => list.toggle(row.path)}
                            />
                          </label>
                        </td>
                        <td className="cv-name" title={row.path}>
                          <b>{row.name}</b>
                          {info && (
                            <span className="cv-meta">
                              {[codecCell(t, info), sizeCell(info), lengthCell(info), formatBytes(info.sizeBytes)].join(" · ")}
                            </span>
                          )}
                          {st.notes.length > 0 && <span>{st.notes.join(" · ")}</span>}
                        </td>
                        <td className="cv-dim cv-wide">{info ? codecCell(t, info) : "—"}</td>
                        <td className="mono cv-dim cv-wide">{info ? sizeCell(info) : "—"}</td>
                        <td className="mono num cv-wide">{info ? lengthCell(info) : "—"}</td>
                        <td className="mono num cv-wide">{info ? formatBytes(info.sizeBytes) : "—"}</td>
                        <td className={`num cv-est ${est.tone}${estimates.stale ? " stale" : ""}`} title={est.title ?? undefined}>
                          {est.text}
                        </td>
                        <td>
                          <span className={`cv-state ${st.tone}`} title={st.why ?? undefined}>
                            {st.label}
                          </span>
                        </td>
                      </tr>
                    );
                  })
                )}
              </tbody>
            </table>
          </div>
          <p className="cv-foot">
            {w.foot}
            {estimates.error && <span className="error"> {w.estimateError(say(t, estimates.error))}</span>}
          </p>
        </main>

        <aside className="ed-aside cv-aside" aria-label={w.settings}>
          <div className="ed-aside-scroll">
          <h2 className="ed-h2">{w.settings}</h2>

          <div className={`cv-ffmpeg${ffmpeg.ready ? "" : " bad"}`}>
            {ffmpeg.ready ? <CheckIcon /> : <WarningIcon />}
            <div>
              <b>
                {ffmpeg.info
                  ? `FFmpeg ${ffmpeg.info.release ?? ffmpeg.info.version} · ${sourceLabel(t, ffmpeg.info.source)}`
                  : ffmpeg.locating
                    ? w.ffmpegLocating
                    : w.ffmpegMissing}
              </b>
              <span className="mono">{ffmpeg.path ?? (ffmpeg.problem ? say(t, ffmpeg.problem) : "")}</span>
            </div>
            <button type="button" className="link small" onClick={onOpenSettings}>
              {w.change}
            </button>
          </div>

          <div className="ed-group" role="group" aria-label={w.outputCodec}>
            <span className="ed-label">{w.outputCodec}</span>
            <div className="ed-seg">
              {codecs(t).map((c) => {
                const missing = missingCodec(c.value);
                return (
                  <button
                    key={c.value}
                    type="button"
                    aria-pressed={choice.codec === c.value}
                    disabled={missing}
                    title={missing ? w.noEncoderFor(c.label) : undefined}
                    onClick={() => setChoice((x) => withCodec(x, c.value))}
                  >
                    <b>{c.label}</b>
                    <span>{c.use}</span>
                  </button>
                );
              })}
            </div>
          </div>

          <div className="ed-group">
            <label className="ed-label" htmlFor="cv-encoder">
              {w.encoder}
            </label>
            <select
              id="cv-encoder"
              className="cv-select"
              value={choice.encoder ?? ""}
              disabled={!encoders.options}
              onChange={(e) => set({ encoder: e.target.value || null })}
            >
              <option value="">
                {encoders.options?.auto[choice.codec] ? w.autoWith(encoderLabel(t, encoders.options.auto[choice.codec]!)) : w.auto}
              </option>
              {encodersFor(encoders.options, choice.codec).map((e) => (
                <option key={e.name} value={e.name}>
                  {encoderLabel(t, e)}
                </option>
              ))}
            </select>
            <div className="cv-row">
              <span className="ed-help">
                {encoders.loading
                  ? t.exportPanel.checkingEncoders
                  : encoders.error
                    ? say(t, encoders.error)
                    : encoderNote(t, encoders.options, choice)}
              </span>
              <button type="button" className="link small" disabled={!ffmpeg.ready || encoders.loading} onClick={encoders.retest}>
                {w.retest}
              </button>
            </div>
          </div>

          <div className="ed-group" role="group" aria-label={t.exportPanel.quality}>
            <span className="ed-label">{t.exportPanel.quality}</span>
            <div className="ed-seg">
              {qualities(t).map((q) => (
                <button key={q.value} type="button" aria-pressed={choice.quality === q.value} onClick={() => set({ quality: q.value })}>
                  <b>{q.label}</b>
                  <span>{q.use}</span>
                </button>
              ))}
            </div>
            <span className="ed-help">{qualityNote(t, choice.quality)}</span>
          </div>

          <div className="ed-group">
            <button type="button" className="cv-adv-toggle" aria-expanded={showAdvanced} onClick={() => setShowAdvanced((s) => !s)}>
              {w.advanced}
              <span>{showAdvanced ? w.collapse : w.expand}</span>
            </button>
            {showAdvanced && (
              <div className="cv-adv">
                <div className="cv-grid">
                  <div className="ed-group">
                    <label className="ed-label" htmlFor="cv-container">
                      {w.container}
                    </label>
                    <select
                      id="cv-container"
                      className="cv-select"
                      value={choice.container}
                      onChange={(e) => set({ container: e.target.value as ConvertChoice["container"] })}
                    >
                      {containersFor(choice.codec).map((c) => (
                        <option key={c} value={c} disabled={c === "webm" && webmWithoutOpus}>
                          {CONTAINER_LABELS[c]}
                          {c === "webm" && webmWithoutOpus ? ` ${w.noLibopus}` : ""}
                        </option>
                      ))}
                    </select>
                  </div>
                  <div className="ed-group">
                    <label className="ed-label" htmlFor="cv-resolution">
                      {w.resolution}
                    </label>
                    <select
                      id="cv-resolution"
                      className="cv-select"
                      value={choice.resolution}
                      onChange={(e) => set({ resolution: e.target.value as ConvertChoice["resolution"] })}
                    >
                      {resolutions(t).map((r) => (
                        <option key={r.value} value={r.value}>
                          {r.label}
                        </option>
                      ))}
                    </select>
                  </div>
                </div>
                <div className="ed-group">
                  <label className="ed-label" htmlFor="cv-audio">
                    {w.audio}
                  </label>
                  <select
                    id="cv-audio"
                    className="cv-select"
                    value={choice.audio}
                    onChange={(e) => set({ audio: e.target.value as ConvertChoice["audio"] })}
                  >
                    {audioChoices(t).map((a) => (
                      <option key={a.value} value={a.value}>
                        {a.label}
                      </option>
                    ))}
                  </select>
                  <span className="ed-help">
                    {choice.container === "webm" ? w.audioWebm : w.audioCopy}
                  </span>
                </div>
                <div className="cv-checks">
                  <label>
                    <input type="checkbox" checked={choice.tenBit} onChange={() => set({ tenBit: !choice.tenBit })} />
                    <span>
                      {t.queueView.tenBit} <span className="ed-help">· {w.tenBitAuto}</span>
                    </span>
                  </label>
                  <label>
                    <input type="checkbox" checked={choice.skipSame} onChange={() => set({ skipSame: !choice.skipSame })} />
                    <span>{w.skipSame}</span>
                  </label>
                </div>
                {choice.codec === "hevc" && choice.container !== "mkv" && (
                  <span className="ed-help">{w.hvc1(CONTAINER_LABELS[choice.container])}</span>
                )}
              </div>
            )}
          </div>

          <div className="ed-save cv-save">
            <div className="cv-row">
              <span className="ed-label">{choice.outputDir ? w.saveChosen : w.saveNext}</span>
              <span className="cv-save-actions">
                {choice.outputDir && (
                  <button type="button" className="link small" onClick={() => set({ outputDir: null })}>
                    {w.toOriginalFolder}
                  </button>
                )}
                <button type="button" className="link small" onClick={() => void pickOutputDir()}>
                  {w.change}
                </button>
              </span>
            </div>
            {choice.outputDir && (
              <span className="mono cv-path" title={choice.outputDir}>
                {choice.outputDir}
              </span>
            )}
            <label className="cv-name-rule">
              <span className="mono">{t.convert.namePlaceholder}</span>
              <input
                className="ed-input mono"
                aria-label={w.suffix}
                aria-invalid={suffixWhy !== null}
                value={suffixOf(choice)}
                placeholder={w.suffixEmpty}
                spellCheck={false}
                onChange={(e) => set({ suffix: e.target.value === defaultSuffix(choice.codec) ? null : e.target.value })}
              />
              <span className="mono">.{choice.container}</span>
            </label>
            <span className={suffixWhy ? "error small" : "ed-label"}>
              {suffixWhy ?? w.nameRule(outputPattern(t, choice))}
            </span>
          </div>

          </div>

          <div className="ed-aside-foot cv-start">
            <div className="cv-totals">
              <span className="cv-total">{totalsLine(t, totals)}</span>
              {estimate && (
                <span className={`cv-est-total${estimate.saving ? " mono" : ""}${estimates.stale ? " stale" : ""}`}>
                  {estimate.text}
                  {estimate.saving && <span className="cv-saving"> ({estimate.saving})</span>}
                </span>
              )}
            </div>
            {estimate?.note && <span className="ed-help">{estimate.note}</span>}
            <button type="button" className="ed-export" disabled={totals.count === 0 || starting || blocked !== null} title={blocked ?? undefined} onClick={() => void start()}>
              {starting ? t.exportPanel.queuing : w.start(totals.count)}
            </button>
            {blocked && totals.count > 0 && <span className="ed-help">{blocked}</span>}
            {started && started.queued.length > 0 && (
              <p className="ed-help">
                {w.queued(started.queued.length)}{" "}
                <button type="button" className="link" onClick={onOpenQueue}>
                  {t.exportPanel.seeQueue}
                </button>
              </p>
            )}
            {started && started.refused.length > 0 && (
              <div className="error small">
                {w.refused(started.refused.length)}
                <ul className="cv-refused">
                  {started.refused.map((r) => (
                    <li key={r.path}>
                      <b>{nameOf(r.path)}</b> — {say(t, r.error)}
                    </li>
                  ))}
                </ul>
              </div>
            )}
            {startError && <p className="error small">{say(t, startError)}</p>}
          </div>
        </aside>
      </div>
    </div>
  );
}
