/**
 * useSessionStore 兼容门面（stage-2 Task 6 拆分）：实现迁移至 src/stores/session/*
 * （model=会话模型与默认值工厂 / lifecycle=字段生命周期纪律 / registry=会话注册表 /
 * log=日志入表与游标 / rules=规则域 / view=视图耦合与布局 / presets=预设与持久化 /
 * compat=Pinia store 组装），本文件仅保持 import 路径不变——所有组件零改动。
 */
export { useSessionStore, registerParserOnClear } from './session/compat'
export { isPullSession } from './session/model'
export type { Session, CenterView } from './session/compat'
export type { SeqRunState } from './session/compat'
