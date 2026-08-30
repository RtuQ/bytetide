let ctx: AudioContext | null = null

/** 短促告警提示音：两声 880Hz 正弦，WebAudio 合成，无外部资源；失败静默 */
export function playAlertBeep(times = 2) {
  try {
    ctx ??= new AudioContext()
    if (ctx.state === 'suspended') void ctx.resume()
    const t0 = ctx.currentTime
    for (let i = 0; i < times; i++) {
      const osc = ctx.createOscillator()
      const gain = ctx.createGain()
      osc.type = 'sine'
      osc.frequency.value = 880
      const start = t0 + i * 0.18
      gain.gain.setValueAtTime(0, start)
      gain.gain.linearRampToValueAtTime(0.22, start + 0.01)
      gain.gain.exponentialRampToValueAtTime(0.001, start + 0.14)
      osc.connect(gain).connect(ctx.destination)
      osc.start(start)
      osc.stop(start + 0.15)
    }
  } catch {
    /* 音频不可用时静默（无设备权限/自动播放策略等） */
  }
}
