import { GripVertical, Pencil, Plus, RefreshCw, Trash2, X } from "lucide-react";
import { FormEvent, useEffect, useRef, useState } from "react";
import type { ReactNode } from "react";
import {
  AdminApiError,
  buildPingTargetPatch,
  canEnablePingTarget,
  createPingTarget,
  deletePingTarget,
  listPingTargets,
  pingTargetMutationMessage,
  PingTarget,
  targetSortOrder,
  updatePingTarget,
  validatePingTargetHost,
} from "../api/admin";
import { handleAdminError } from "../stores/auth";
import { Button } from "../ui/primitives";

type DialogKind = "create" | "edit" | "delete" | null;

export function AdminPingTargetsPage() {
  const [targets, setTargets] = useState<PingTarget[]>([]);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const [dialog, setDialog] = useState<DialogKind>(null);
  const [selected, setSelected] = useState<PingTarget | null>(null);
  const [busy, setBusy] = useState(false);
  const [draggingId, setDraggingId] = useState<number | null>(null);

  const load = async () => {
    setLoading(true);
    try {
      setTargets((await listPingTargets()).targets);
      setError(null);
    } catch (e) {
      setError(handleAdminError(e));
    } finally {
      setLoading(false);
    }
  };
  useEffect(() => {
    void load();
  }, []);

  const [dialogError, setDialogError] = useState<string | null>(null);
  const mutation = async (
    work: () => Promise<unknown>,
    localError?: (message: string) => void,
  ) => {
    setBusy(true);
    try {
      await work();
      await load();
      setDialog(null);
      setDialogError(null);
    } catch (e) {
      const message = handleAdminError(e);
      if (
        localError &&
        !(e instanceof AdminApiError && (e.status === 401 || e.status === 403))
      )
        localError(
          e instanceof AdminApiError
            ? pingTargetMutationMessage(e.status) ?? message
            : message,
        );
      else setError(message);
    } finally {
      setBusy(false);
    }
  };
  const move = (target: PingTarget, delta: number) => {
    if (busy) return;
    const next = targetSortOrder(target.sort_order, delta, targets.length);
    if (next === null) return;
    void mutation(() => updatePingTarget(target.id, { sort_order: next }));
  };
  const enabledCount = targets.filter((target) => target.enabled).length;

  return (
    <section className="admin-nodes-page">
      <div className="admin-toolbar">
        <h1>延迟监控</h1>
        <Button
          type="button"
          onClick={() => {
            setSelected(null);
            setDialogError(null);
            setDialog("create");
          }}
          icon={<Plus size={17} aria-hidden="true" />}
        >
          添加目标
        </Button>
      </div>
      {error && (
        <div className="admin-error" role="alert">
          <span>{error}</span>
          <button type="button" onClick={() => void load()} aria-label="重试">
            <RefreshCw size={16} aria-hidden="true" />
          </button>
        </div>
      )}
      <div className="admin-table-wrap">
        <table className="admin-table ping-target-table">
          <thead>
            <tr>
              <th aria-label="排序" />
              <th>名称</th>
              <th>目标</th>
              <th>协议</th>
              <th>状态</th>
              <th>操作</th>
            </tr>
          </thead>
          <tbody>
            {loading ? (
              <TableSkeleton />
            ) : (
              targets.map((target) => (
                <TargetRow
                  key={target.id}
                  target={target}
                  busy={busy}
                  isLast={target.sort_order >= targets.length - 1}
                  onMove={(delta) => move(target, delta)}
                  onDragStart={() => setDraggingId(target.id)}
                  onDragEnd={() => setDraggingId(null)}
                  onDrop={() => {
                    if (busy || draggingId === null || draggingId === target.id) {
                      setDraggingId(null);
                      return;
                    }
                    const source = targets.find((item) => item.id === draggingId);
                    setDraggingId(null);
                    if (source) {
                      void mutation(() =>
                        updatePingTarget(source.id, { sort_order: target.sort_order }),
                      );
                    }
                  }}
                  onEdit={() => {
                    setSelected(target);
                    setDialogError(null);
                    setDialog("edit");
                  }}
                  onDelete={() => {
                    setSelected(target);
                    setDialog("delete");
                  }}
                />
              ))
            )}
          </tbody>
        </table>
      </div>
      {!loading && targets.length === 0 && <div className="admin-empty">暂无延迟监控目标</div>}
      {dialog === "create" && (
        <TargetDialog
          title="添加目标"
          busy={busy}
          defaultEnabled={enabledCount < 6}
          enabledCount={enabledCount}
          serverError={dialogError}
          onClose={() => setDialog(null)}
          onSubmit={(body) => mutation(() => createPingTarget(body), setDialogError)}
        />
      )}
      {dialog === "edit" && selected && (
        <TargetDialog
          title="编辑目标"
          target={selected}
          busy={busy}
          serverError={dialogError}
          onClose={() => setDialog(null)}
          onSubmit={async (body) => {
            if (Object.keys(body).length === 0) {
              setDialog(null);
              return;
            }
            await mutation(() => updatePingTarget(selected.id, body), setDialogError);
          }}
          enabledCount={enabledCount}
        />
      )}
      {dialog === "delete" && selected && (
        <ConfirmDialog
          title="删除目标"
          text={`确定删除“${selected.name}”？删除后该目标的历史延迟数据也会永久删除，无法恢复。`}
          busy={busy}
          onClose={() => setDialog(null)}
          onConfirm={() => void mutation(() => deletePingTarget(selected.id))}
        />
      )}
    </section>
  );
}

function TargetRow({
  target,
  busy,
  isLast,
  onMove,
  onDragStart,
  onDragEnd,
  onDrop,
  onEdit,
  onDelete,
}: {
  target: PingTarget;
  busy: boolean;
  isLast: boolean;
  onMove: (delta: number) => void;
  onDragStart: () => void;
  onDragEnd: () => void;
  onDrop: () => void;
  onEdit: () => void;
  onDelete: () => void;
}) {
  return (
    <tr onDragOver={(event) => event.preventDefault()} onDrop={onDrop}>
      <td className="sort-cell">
        <button
          type="button"
          className="drag-handle"
          draggable={!busy}
          disabled={busy}
          aria-label="拖动排序"
          onDragStart={(event) => {
            if (busy) {
              event.preventDefault();
              return;
            }
            onDragStart();
          }}
          onDragEnd={onDragEnd}
          onKeyDown={(event) => {
            if (busy) return;
            if (event.key === "ArrowUp" && target.sort_order > 0) {
              event.preventDefault();
              onMove(-1);
            } else if (event.key === "ArrowDown" && !isLast) {
              event.preventDefault();
              onMove(1);
            }
          }}
        >
          <GripVertical size={17} aria-hidden="true" />
        </button>
      </td>
      <td>
        <strong className="admin-node-name" title={target.name}>{target.name}</strong>
      </td>
      <td className="admin-target-host" title={target.host}>{target.host}</td>
      <td>IPv{target.ip_family}</td>
      <td><span className={`admin-status ${target.enabled ? "online" : "offline"}`}>{target.enabled ? "启用" : "停用"}</span></td>
      <td>
        <div className="row-actions">
          <button type="button" aria-label="编辑" title="编辑" onClick={onEdit} disabled={busy}><Pencil size={18} aria-hidden="true" /></button>
          <button type="button" className="danger" aria-label="删除" title="删除" onClick={onDelete} disabled={busy}><Trash2 size={18} aria-hidden="true" /></button>
        </div>
      </td>
    </tr>
  );
}

function TargetDialog({
  title,
  target,
  busy,
  defaultEnabled = true,
  enabledCount = 0,
  serverError,
  onClose,
  onSubmit,
}: {
  title: string;
  target?: PingTarget;
  busy: boolean;
  defaultEnabled?: boolean;
  enabledCount?: number;
  onClose: () => void;
  onSubmit: (body: Record<string, unknown>) => Promise<void>;
  serverError?: string | null;
}) {
  const [name, setName] = useState(target?.name ?? "");
  const [host, setHost] = useState(target?.host ?? "");
  const [family, setFamily] = useState<4 | 6>(target?.ip_family ?? 4);
  const [enabled, setEnabled] = useState(target?.enabled ?? defaultEnabled);
  const [formError, setFormError] = useState<string | null>(null);
  const submit = async (event: FormEvent) => {
    event.preventDefault();
    const normalizedName = name.trim();
    const normalizedHost = host.trim();
    if (normalizedName.length < 1 || normalizedName.length > 64) {
      setFormError("名称长度需为 1–64 个字符");
      return;
    }
    if (!validatePingTargetHost(normalizedHost)) {
      setFormError("目标地址格式无效");
      return;
    }
    if (!target && enabled && !canEnablePingTarget(enabledCount, false)) {
      setFormError("最多只能启用 6 个延迟监控目标");
      return;
    }
    if (target && enabled && !canEnablePingTarget(enabledCount, target.enabled)) {
      setFormError("最多只能启用 6 个延迟监控目标");
      return;
    }
    const form = { name: normalizedName, host: normalizedHost, ip_family: family, enabled };
    await onSubmit(target ? buildPingTargetPatch(target, form) : form);
  };
  return (
    <Dialog title={title} onClose={onClose}>
      <form onSubmit={submit} className="node-form">
        <label>名称<input value={name} onChange={(event) => setName(event.target.value)} autoFocus /></label>
        <label>目标地址<input value={host} onChange={(event) => setHost(event.target.value)} placeholder="hostname / IP" /></label>
        <label>IP 协议<select value={family} onChange={(event) => setFamily(Number(event.target.value) as 4 | 6)}><option value={4}>IPv4</option><option value={6}>IPv6</option></select></label>
        <label className="checkbox-label"><input type="checkbox" checked={enabled} onChange={(event) => setEnabled(event.target.checked)} />启用</label>
        {(formError || serverError) && <div className="form-error" role="alert">{formError ?? serverError}</div>}
        <div className="dialog-actions"><Button type="button" onClick={onClose}>取消</Button><Button type="submit" disabled={busy}>{busy ? "保存中…" : "保存"}</Button></div>
      </form>
    </Dialog>
  );
}

function TableSkeleton() {
  return <>{Array.from({ length: 4 }, (_, index) => <tr key={index} className="admin-skeleton-row"><td colSpan={6}><span /></td></tr>)}</>;
}

function Dialog({ title, children, onClose }: { title: string; children: ReactNode; onClose: () => void }) {
  const dialog = useRef<HTMLDialogElement>(null);
  useEffect(() => {
    dialog.current?.showModal();
    return () => { if (dialog.current?.open) dialog.current.close(); };
  }, []);
  return <dialog ref={dialog} className="admin-dialog" aria-labelledby="dialog-title" onCancel={(event) => { event.preventDefault(); onClose(); }}>
    <button type="button" className="dialog-close" aria-label="关闭" onClick={onClose}><X size={18} aria-hidden="true" /></button>
    <h2 id="dialog-title">{title}</h2>
    {children}
  </dialog>;
}

function ConfirmDialog({ title, text, busy, onClose, onConfirm }: { title: string; text: string; busy: boolean; onClose: () => void; onConfirm: () => void }) {
  return <Dialog title={title} onClose={onClose}>
    <p className="confirm-text">{text}</p>
    <div className="dialog-actions"><Button type="button" onClick={onClose}>取消</Button><Button type="button" className="danger-button" onClick={onConfirm} disabled={busy}>{busy ? "处理中…" : "确认"}</Button></div>
  </Dialog>;
}
