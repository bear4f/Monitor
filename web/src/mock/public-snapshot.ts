import type { PublicNode, PublicSnapshot } from "../api/public";

const GENERATED_AT = 1_788_664_402;
const GB = 1024 ** 3;
const TB = 1024 ** 4;

function makeNode(
  index: number,
  name: string,
  region: string,
  cpu: number,
  online: boolean,
  limit: number | null,
  expiresAt: number | null,
): PublicNode {
  const memoryTotal = (1 + (index % 4)) * GB;
  const diskTotal = (20 + index * 10) * GB;
  const cycleRx = (8 + index * 3) * GB;
  const cycleTx = (5 + index * 2) * GB;
  return {
    id: index.toString(16).padStart(32, "0"),
    name,
    region_code: region,
    sort_order: index - 1,
    online,
    first_seen_at: GENERATED_AT - (index + 2) * 864_000,
    last_seen_at: GENERATED_AT - (online ? index : 3_600),
    system: {
      hostname: `${name.toLowerCase()}-01`,
      os_name: index === 7 ? "Alpine Linux" : "Debian",
      os_version: index === 7 ? "3.24" : index % 2 === 0 ? "12" : "13",
      kernel: "6.12.107-amd64",
      architecture: index === 6 ? "aarch64" : "x86_64",
      virtualization: index % 3 === 0 ? "kvm" : "qemu",
      agent_version: "0.1.0",
      cpu_model: "AMD EPYC 9654",
      cpu_cores: index === 3 ? 4 : 1,
      process_count: 74 + index * 5,
      uptime_seconds: index * 92_400,
    },
    metrics: {
      cpu_usage: cpu,
      load_1: cpu / 28,
      load_5: cpu / 34,
      load_15: cpu / 40,
      memory_total: memoryTotal,
      memory_used: Math.round(memoryTotal * (0.18 + index * 0.045)),
      swap_total: index % 2 === 0 ? 0 : GB,
      swap_used: index % 2 === 0 ? 0 : index * 8 * 1024 ** 2,
      disk_total: diskTotal,
      disk_used: Math.round(diskTotal * (0.08 + index * 0.035)),
      current_rx_rate: online ? 200 + index * 480 : 0,
      current_tx_rate: online ? 120 + index * 150 : 0,
    },
    traffic: {
      today_rx: (1 + index) * 420 * 1024 ** 2,
      today_tx: (1 + index) * 260 * 1024 ** 2,
      cycle_rx: cycleRx,
      cycle_tx: cycleTx,
      total_rx: (300 + index * 140) * GB,
      total_tx: (260 + index * 120) * GB,
      limit,
      cycle_start_at: GENERATED_AT - 6 * 86400,
      cycle_end_at: GENERATED_AT + 24 * 86400,
    },
    billing: expiresAt === null && index % 2 === 0 ? null : {
      price_micros: index === 4 ? 0 : 39_900_000,
      currency: "USD",
      renewal_cycle: "annual",
      expires_at: expiresAt,
    },
  };
}

const nodes = [
  makeNode(1, "DMIT", "US", 0.8, true, TB, GENERATED_AT + 60 * 86400),
  makeNode(2, "Zouter", "JP", 2.4, true, 2 * TB, GENERATED_AT + 318 * 86400),
  makeNode(3, "CCS", "HK", 72.4, true, 40 * TB, GENERATED_AT + 446 * 86400),
  makeNode(4, "Cloud Cone", "US", 8.6, true, 3 * TB, null),
  makeNode(5, "Frankfurt", "DE", 4.2, false, 2 * TB, null),
  makeNode(6, "London", "GB", 12.6, true, null, null),
  makeNode(7, "Alice", "SG", 1.1, true, null, GENERATED_AT + 3 * 86400),
  makeNode(8, "Sydney", "AU", 6.3, true, 4 * TB, GENERATED_AT - 3 * 86400),
];

const total = (field: "today_rx" | "today_tx" | "total_rx" | "total_tx") =>
  nodes.reduce((sum, node) => sum + node.traffic[field], 0);

const timestamps = Array.from({ length: 30 }, (_, index) => GENERATED_AT - (29 - index) * 2);
const rx = timestamps.map((_, index) => Math.round(16_000 + index * 35 + Math.sin(index / 4) * 420));
const tx = timestamps.map((_, index) => Math.round(5_200 + index * 18 + Math.sin(index / 5) * 180));

export const mockPublicSnapshot: PublicSnapshot = {
  generated_at: GENERATED_AT,
  site: {
    name: "Monitor",
    timezone: "Asia/Shanghai",
    theme_default: "system",
  },
  summary: {
    online_nodes: nodes.filter((node) => node.online).length,
    total_nodes: nodes.length,
    busiest_node: { id: nodes[2].id, name: nodes[2].name, cpu_usage: nodes[2].metrics?.cpu_usage ?? 0 },
    today_rx: total("today_rx"),
    today_tx: total("today_tx"),
    total_rx: total("total_rx"),
    total_tx: total("total_tx"),
    current_rx_rate: nodes.reduce((sum, node) => sum + (node.online ? node.metrics?.current_rx_rate ?? 0 : 0), 0),
    current_tx_rate: nodes.reduce((sum, node) => sum + (node.online ? node.metrics?.current_tx_rate ?? 0 : 0), 0),
    network_rate_history: { timestamps, rx, tx },
  },
  nodes,
};
