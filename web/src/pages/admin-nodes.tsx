import {
  Check,
  Copy,
  GripVertical,
  Pencil,
  Plus,
  RefreshCw,
  RotateCw,
  Trash2,
  X,
} from "lucide-react";
import { FormEvent, useEffect, useMemo, useRef, useState } from "react";
import type { ReactNode } from "react";
import {
  AdminNode,
  buildNodePatch,
  buildRuntimeConfig,
  createNode,
  deleteNode,
  expiryEpochToLocalDate,
  formatMicrosForInput,
  listNodes,
  localDateToExpiryEpoch,
  normalizeCode,
  parseMoneyToMicros,
  RENEWAL_CYCLES,
  rotateToken,
  trafficUnitBytes,
  updateNode,
} from "../api/admin";
import {
  formatBytes,
  formatCalendarDate,
  formatPrice,
  safeAdd,
} from "../lib/format";
import { handleAdminError } from "../stores/auth";
import { Button } from "../ui/primitives";

type DialogKind = "create" | "edit" | "delete" | "rotate" | "token" | null;
const CYCLE_LABELS: Record<string, string> = {
  monthly: "月付",
  quarterly: "季付",
  semiannual: "半年付",
  annual: "年付",
  biennial: "两年付",
  custom: "自定义",
};
const moneyLabel = (node: AdminNode) => {
  if (node.price_micros === null || node.currency === null) return "—";
  const cycle = node.renewal_cycle
    ? ` / ${CYCLE_LABELS[node.renewal_cycle]}`
    : "";
  return `${formatPrice(node.price_micros, node.currency)}${cycle}`;
};
const now = () => Math.floor(Date.now() / 1000);

export function AdminNodesPage() {
  const [nodes, setNodes] = useState<AdminNode[]>([]);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const [dialog, setDialog] = useState<DialogKind>(null);
  const [selected, setSelected] = useState<AdminNode | null>(null);
  const [token, setToken] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const [draggingId, setDraggingId] = useState<string | null>(null);
  const load = async () => {
    setLoading(true);
    try {
      setNodes((await listNodes()).nodes);
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
  const mutation = async (work: () => Promise<unknown>, close = true) => {
    setBusy(true);
    try {
      await work();
      await load();
      if (close) setDialog(null);
    } catch (e) {
      setError(handleAdminError(e));
    } finally {
      setBusy(false);
    }
  };
  const move = (node: AdminNode, delta: number) =>
    mutation(() =>
      updateNode(node.id, { sort_order: Math.max(0, node.sort_order + delta) }),
    );
  return (
    <section className="admin-nodes-page">
      <div className="admin-toolbar">
        <h1>节点</h1>
        <Button
          type="button"
          onClick={() => {
            setSelected(null);
            setDialog("create");
          }}
          icon={<Plus size={17} aria-hidden="true" />}
        >
          添加节点
        </Button>
      </div>
      {error && (
        <div className="admin-error" role="alert">
          <span>{error}</span>
          <button type="button" onClick={() => void load()} aria-label="重试">
            <RefreshCw size={16} />
          </button>
        </div>
      )}
      <div className="admin-table-wrap">
        <table className="admin-table">
          <thead>
            <tr>
              <th aria-label="排序" />
              <th>名称</th>
              <th>IP</th>
              <th>状态</th>
              <th>流量</th>
              <th>价格</th>
              <th>到期</th>
              <th>操作</th>
            </tr>
          </thead>
          <tbody>
            {loading ? (
              <TableSkeleton />
            ) : (
              nodes.map((node) => (
                <NodeRow
                  key={node.id}
                  node={node}
                  busy={busy}
                  isLast={node.sort_order >= nodes.length - 1}
                  onMove={(delta) => void move(node, delta)}
                  onDragStart={() => setDraggingId(node.id)}
                  onDragEnd={() => setDraggingId(null)}
                  onDrop={() => {
                    if (!draggingId || draggingId === node.id) return;
                    const source = nodes.find((item) => item.id === draggingId);
                    if (source)
                      void mutation(() =>
                        updateNode(source.id, { sort_order: node.sort_order }),
                      );
                    setDraggingId(null);
                  }}
                  onEdit={() => {
                    setSelected(node);
                    setDialog("edit");
                  }}
                  onDelete={() => {
                    setSelected(node);
                    setDialog("delete");
                  }}
                  onRotate={() => {
                    setSelected(node);
                    setDialog("rotate");
                  }}
                />
              ))
            )}
          </tbody>
        </table>
      </div>
      {!loading && nodes.length === 0 && (
        <div className="admin-empty">暂无节点</div>
      )}
      {dialog === "create" && (
        <NodeDialog
          title="添加节点"
          onClose={() => setDialog(null)}
          busy={busy}
          onSubmit={(body) =>
            mutation(async () => {
              const result = await createNode(body);
              setToken(result.agent_token);
              setDialog("token");
            }, false)
          }
        />
      )}
      {dialog === "edit" && selected && (
        <NodeDialog
          title="编辑节点"
          node={selected}
          onClose={() => setDialog(null)}
          busy={busy}
          onSubmit={(body) => {
            if (Object.keys(body).length === 0) {
              setDialog(null);
              return;
            }
            void mutation(() => updateNode(selected.id, body));
          }}
        />
      )}
      {dialog === "delete" && selected && (
        <ConfirmDialog
          title="删除节点"
          text={`确定删除“${selected.name}”？节点历史与流量数据将被永久删除，无法恢复。`}
          busy={busy}
          onClose={() => setDialog(null)}
          onConfirm={() => mutation(() => deleteNode(selected.id))}
        />
      )}
      {dialog === "rotate" && selected && (
        <ConfirmDialog
          title="轮换 Agent Token"
          text="轮换后当前 Agent Token 立即失效，当前 Agent 会停止上报，必须重新配置 Agent。"
          busy={busy}
          onClose={() => setDialog(null)}
          onConfirm={() =>
            mutation(async () => {
              const result = await rotateToken(selected.id);
              setToken(result.agent_token);
              setDialog("token");
            }, false)
          }
        />
      )}
      {dialog === "token" && token && (
        <TokenDialog
          token={token}
          onClose={() => {
            setToken(null);
            setDialog(null);
          }}
        />
      )}
    </section>
  );
}

function NodeRow({
  node,
  busy,
  isLast,
  onMove,
  onDragStart,
  onDragEnd,
  onDrop,
  onEdit,
  onDelete,
  onRotate,
}: {
  node: AdminNode;
  busy: boolean;
  isLast: boolean;
  onMove: (delta: number) => void;
  onDragStart: () => void;
  onDragEnd: () => void;
  onDrop: () => void;
  onEdit: () => void;
  onDelete: () => void;
  onRotate: () => void;
}) {
  const used = safeAdd(node.cycle_rx, node.cycle_tx);
  const percent =
    used !== null && node.traffic_limit
      ? Math.min(100, (used / node.traffic_limit) * 100)
      : null;
  return (
    <tr onDragOver={(event) => event.preventDefault()} onDrop={onDrop}>
      <td className="sort-cell">
        <button
          type="button"
          className="drag-handle"
          draggable={!busy}
          disabled={busy}
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
            if (event.key === "ArrowUp" && node.sort_order > 0) {
              event.preventDefault();
              onMove(-1);
            }
            if (event.key === "ArrowDown" && !isLast) {
              event.preventDefault();
              onMove(1);
            }
          }}
          aria-label="拖动排序"
        >
          <GripVertical size={17} aria-hidden="true" />
        </button>
      </td>
      <td>
        <strong className="admin-node-name" title={node.name}>
          {node.name}
        </strong>
        <span className="admin-region">{node.region_code}</span>
      </td>
      <td>{node.last_ip ?? "—"}</td>
      <td>
        <span className={`admin-status ${node.online ? "online" : "offline"}`}>
          {node.online ? "在线" : "离线"}
        </span>
      </td>
      <td>
        <div>
          {used === null ? "—" : formatBytes(used)} /{" "}
          {node.traffic_limit === null ? "∞" : formatBytes(node.traffic_limit)}
        </div>
        {node.traffic_limit !== null && percent !== null && (
          <div className="admin-progress">
            <i style={{ width: `${percent}%` }} />
          </div>
        )}
      </td>
      <td>{moneyLabel(node)}</td>
      <td>
        {node.expires_at === null
          ? "∞"
          : node.expires_at <= now()
            ? "已过期"
            : formatCalendarDate(node.expires_at)}
      </td>
      <td>
        <div className="row-actions">
          <button
            type="button"
            aria-label="Token/Agent 配置"
            title="Token/Agent 配置"
            onClick={onRotate}
            disabled={busy}
          >
            <RotateCw size={18} />
          </button>
          <button
            type="button"
            aria-label="编辑"
            title="编辑"
            onClick={onEdit}
            disabled={busy}
          >
            <Pencil size={18} />
          </button>
          <button
            type="button"
            className="danger"
            aria-label="删除"
            title="删除"
            onClick={onDelete}
            disabled={busy}
          >
            <Trash2 size={18} />
          </button>
        </div>
      </td>
    </tr>
  );
}
function TableSkeleton() {
  return (
    <>
      {Array.from({ length: 4 }, (_, i) => (
        <tr key={i} className="admin-skeleton-row">
          <td colSpan={8}>
            <span />
          </td>
        </tr>
      ))}
    </>
  );
}

function NodeDialog({
  title,
  node,
  busy,
  onClose,
  onSubmit,
}: {
  title: string;
  node?: AdminNode;
  busy: boolean;
  onClose: () => void;
  onSubmit: (body: Record<string, unknown>) => void;
}) {
  const unit = "GB";
  const [name, setName] = useState(node?.name ?? "");
  const [region, setRegion] = useState(node?.region_code ?? "");
  const [limit, setLimit] = useState(
    node?.traffic_limit === null || node?.traffic_limit === undefined
      ? ""
      : String(node.traffic_limit / 1024 ** 3),
  );
  const [resetDay, setResetDay] = useState("");
  const [price, setPrice] = useState(
    node?.price_micros === null || node?.price_micros === undefined
      ? ""
      : formatMicrosForInput(node.price_micros),
  );
  const [currency, setCurrency] = useState(node?.currency ?? "");
  const [renewal, setRenewal] = useState(node?.renewal_cycle ?? "");
  const [expires, setExpires] = useState(
    expiryEpochToLocalDate(node?.expires_at ?? null),
  );
  const [formError, setFormError] = useState<string | null>(null);
  const submit = (event: FormEvent) => {
    event.preventDefault();
    const normalized = normalizeCode(region, 2);
    const micros = price === "" ? null : parseMoneyToMicros(price);
    const traffic = limit === "" ? null : trafficUnitBytes(limit, unit);
    const day = resetDay === "" ? null : Number(resetDay);
    const expiry = expires === "" ? null : localDateToExpiryEpoch(expires);
    if (!name.trim() || name.trim().length > 64)
      return setFormError("名称长度需为 1–64 个字符");
    if (!normalized) return setFormError("地区必须是两个字母");
    if (price !== "" && micros === null) return setFormError("价格格式无效");
    if (limit !== "" && traffic === null) return setFormError("流量额度无效");
    if (
      !node &&
      resetDay !== "" &&
      (!Number.isInteger(day) || day! < 1 || day! > 31)
    )
      return setFormError("重置日需为 1–31");
    if (expires !== "" && expiry === null) return setFormError("日期格式无效");
    const form = {
      name: name.trim(),
      region_code: normalized,
      traffic_limit: traffic,
      price_micros: micros,
      currency: micros === null ? null : normalizeCode(currency, 3),
      renewal_cycle:
        renewal === "" ? null : (renewal as AdminNode["renewal_cycle"]),
      expires_at: expiry,
    };
    if (form.price_micros !== null && !form.currency)
      return setFormError("货币必须是三个字母");
    if (node) onSubmit(buildNodePatch(node, form));
    else
      onSubmit({
        ...form,
        ...(day === null ? {} : { traffic_reset_day: day }),
      });
  };
  return (
    <Dialog title={title} onClose={onClose}>
      <form onSubmit={submit} className="node-form">
        <label>
          名称
          <input
            value={name}
            onChange={(e) => setName(e.target.value)}
            autoFocus
          />
        </label>
        <label>
          地区代码
          <input
            value={region}
            onChange={(e) => setRegion(e.target.value)}
            maxLength={2}
          />
        </label>
        <label>
          流量额度
          <div className="inline-fields">
            <input
              type="number"
              step="any"
              value={limit}
              onChange={(e) => setLimit(e.target.value)}
              placeholder="无限"
            />
            <select value={unit} disabled>
              <option>GB</option>
            </select>
          </div>
        </label>
        {!node && (
          <label>
            流量重置日
            <input
              type="number"
              min="1"
              max="31"
              value={resetDay}
              onChange={(e) => setResetDay(e.target.value)}
              placeholder="使用服务器默认"
            />
          </label>
        )}
        <label>
          价格
          <input
            value={price}
            onChange={(e) => setPrice(e.target.value)}
            placeholder="留空表示无价格"
            inputMode="decimal"
          />
        </label>
        <label>
          货币
          <input
            value={currency}
            onChange={(e) => setCurrency(e.target.value)}
            maxLength={3}
          />
        </label>
        <label>
          续费周期
          <select value={renewal} onChange={(e) => setRenewal(e.target.value)}>
            <option value="">无</option>
            {RENEWAL_CYCLES.map((value) => (
              <option key={value} value={value}>
                {CYCLE_LABELS[value]}
              </option>
            ))}
          </select>
        </label>
        <label>
          到期日期
          <input
            type="date"
            value={expires}
            onChange={(e) => setExpires(e.target.value)}
          />
        </label>
        {formError && (
          <div className="form-error" role="alert">
            {formError}
          </div>
        )}
        <div className="dialog-actions">
          <Button type="button" onClick={onClose}>
            取消
          </Button>
          <Button type="submit" disabled={busy}>
            {busy ? "保存中…" : "保存"}
          </Button>
        </div>
      </form>
    </Dialog>
  );
}
function Dialog({
  title,
  children,
  onClose,
}: {
  title: string;
  children: ReactNode;
  onClose: () => void;
}) {
  const dialog = useRef<HTMLDialogElement>(null);
  useEffect(() => {
    dialog.current?.showModal();
    return () => {
      if (dialog.current?.open) dialog.current.close();
    };
  }, []);
  return (
    <dialog
      ref={dialog}
      className="admin-dialog"
      aria-labelledby="dialog-title"
      onCancel={(event) => {
        event.preventDefault();
        onClose();
      }}
    >
      <button
        type="button"
        className="dialog-close"
        aria-label="关闭"
        onClick={onClose}
      >
        <X size={18} />
      </button>
      <h2 id="dialog-title">{title}</h2>
      {children}
    </dialog>
  );
}
function ConfirmDialog({
  title,
  text,
  busy,
  onClose,
  onConfirm,
}: {
  title: string;
  text: string;
  busy: boolean;
  onClose: () => void;
  onConfirm: () => void;
}) {
  return (
    <Dialog title={title} onClose={onClose}>
      <p className="confirm-text">{text}</p>
      <div className="dialog-actions">
        <Button type="button" onClick={onClose}>
          取消
        </Button>
        <Button
          type="button"
          className="danger-button"
          onClick={onConfirm}
          disabled={busy}
        >
          {busy ? "处理中…" : "确认"}
        </Button>
      </div>
    </Dialog>
  );
}
function TokenDialog({
  token,
  onClose,
}: {
  token: string;
  onClose: () => void;
}) {
  const [copied, setCopied] = useState(false);
  const config = useMemo(
    () => buildRuntimeConfig(window.location.origin, token),
    [token],
  );
  const copy = async (text: string) => {
    try {
      await navigator.clipboard.writeText(text);
      setCopied(true);
      setTimeout(() => setCopied(false), 1200);
    } catch {
      /* manual copy remains available */
    }
  };
  return (
    <Dialog title="一次性 Agent Token" onClose={onClose}>
      <p className="token-warning">
        关闭后无法再次查看。丢失后只能轮换 Token。
      </p>
      <code className="token-value">{token}</code>
      <button
        type="button"
        className="copy-button"
        onClick={() => void copy(token)}
      >
        {copied ? <Check size={16} /> : <Copy size={16} />}{" "}
        {copied ? "已复制" : "复制 Token"}
      </button>
      <pre className="runtime-config">{config}</pre>
      <button
        type="button"
        className="copy-button"
        onClick={() => void copy(config)}
      >
        <Copy size={16} />
        复制配置
      </button>
      <div className="dialog-actions">
        <Button type="button" onClick={onClose}>
          关闭
        </Button>
      </div>
    </Dialog>
  );
}
