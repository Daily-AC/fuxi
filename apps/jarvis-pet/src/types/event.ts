/// fuxi EventKind wire 形式（按 #[serde(tag="type")] tagged union）
/// 仅列 jarvis-pet 关心的子集；其它走 WireKind.other
export type WireKind =
  | { type: 'xuannv_voice_line'; text: string; emotion?: string | null }
  | { type: 'thinking_started' }
  | { type: 'thinking_finished' }
  | { type: 'agent_responded'; text: string }
  | { type: 'task_dispatched'; to: string }
  | { type: 'task_state_changed'; to: string }
  | { type: 'deliverable_accepted'; deliverable_id: string }
  | { type: 'deliverable_produced'; deliverable_kind: string; files: unknown[] }
  | { type: 'agent_dead'; cause: string }
  | { type: 'worker_heartbeat_state_changed'; inflight_count: number; max_concurrency: number }
  | { type: 'worker_stale_swept'; recycled_jobs: unknown[] }
  | { type: 'usage_report'; total_tokens: number; window_size: number; pct: number }
  | { type: 'xuannv_context_watermark'; threshold_pct: number; action: string }
  | { type: 'xuannv_handoff_written'; path: string; length_chars: number }
  | { type: 'user_prompted'; text: string }
  | { type: string; [key: string]: unknown }   // unknown fallback

export interface WireEvent {
  meta: {
    id?: string
    agent?: string
    task?: string
  }
  kind: WireKind
}
