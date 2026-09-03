/**
 * 解析引擎校验算法集（framing.crc 与 Worker 内 bt.crc 共用同一实现）。
 * 参数按 reveng catalog 钉死（docs/parser-spec.md 附参数表与 "123456789" 校验值）。
 *
 * 注意：crcSum8/crcXor8/crc16Generic/crc32 必须**自包含**（不引用任何外部标识符）——
 * bootstrap.ts 以 Function.prototype.toString() 把它们内嵌进 Worker 源码，
 * 修改函数体时不得引入外部依赖（含 import 的常量/辅助函数）。
 */
/** 累加和低 8 位 */
export function crcSum8(bytes: Uint8Array): number {
  let s = 0
  for (let i = 0; i < bytes.length; i++) s = (s + bytes[i]) & 0xff
  return s
}

/** 逐字节异或 */
export function crcXor8(bytes: Uint8Array): number {
  let x = 0
  for (let i = 0; i < bytes.length; i++) x ^= bytes[i]
  return x
}

/**
 * 通用 CRC16（MSB-first 移位实现，字节序反射在函数内完成）。
 * poly 为 MSB 形式（如 modbus 0x8005）；refin/refout 控制输入/输出反射。
 */
export function crc16Generic(
  bytes: Uint8Array,
  poly: number,
  init: number,
  refin: boolean,
  refout: boolean,
  xorout: number,
): number {
  let crc = init & 0xffff
  for (let i = 0; i < bytes.length; i++) {
    let b = bytes[i] & 0xff
    if (refin) {
      b =
        (((b & 1) << 7) |
          ((b & 2) << 5) |
          ((b & 4) << 3) |
          ((b & 8) << 1) |
          ((b >> 1) & 8) |
          ((b >> 3) & 4) |
          ((b >> 5) & 2) |
          ((b >> 7) & 1)) &
        0xff
    }
    crc ^= b << 8
    for (let k = 0; k < 8; k++) {
      crc = (crc & 0x8000) !== 0 ? ((crc << 1) ^ poly) & 0xffff : (crc << 1) & 0xffff
    }
  }
  if (refout) {
    crc =
      (((crc & 0x0001) << 15) |
        ((crc & 0x0002) << 13) |
        ((crc & 0x0004) << 11) |
        ((crc & 0x0008) << 9) |
        ((crc & 0x0010) << 7) |
        ((crc & 0x0020) << 5) |
        ((crc & 0x0040) << 3) |
        ((crc & 0x0080) << 1) |
        ((crc >> 1) & 0x0080) |
        ((crc >> 3) & 0x0040) |
        ((crc >> 5) & 0x0020) |
        ((crc >> 7) & 0x0010) |
        ((crc >> 9) & 0x0008) |
        ((crc >> 11) & 0x0004) |
        ((crc >> 13) & 0x0002) |
        ((crc >> 15) & 0x0001)) &
      0xffff
  }
  return (crc ^ xorout) & 0xffff
}

/** CRC32（IEEE，zlib 同款）：poly 0x04C11DB7 反射形式，init/xorout 0xFFFFFFFF */
export function crc32(bytes: Uint8Array): number {
  let crc = 0xffffffff
  for (let i = 0; i < bytes.length; i++) {
    crc = (crc ^ (bytes[i] & 0xff)) & 0xffffffff
    for (let k = 0; k < 8; k++) {
      crc = (crc & 1) !== 0 ? ((crc >>> 1) ^ 0xedb88320) >>> 0 : crc >>> 1
    }
  }
  return (crc ^ 0xffffffff) >>> 0
}

/** reveng catalog 参数表：算法 → [poly, init, refin, refout, xorout]（crc32 除外） */
export const CRC16_PARAMS: Record<
  Exclude<BytetideParser.CrcAlgo, 'sum8' | 'xor8' | 'crc32'>,
  [number, number, boolean, boolean, number]
> = {
  'crc16-modbus': [0x8005, 0xffff, true, true, 0x0000],
  'crc16-ccitt-false': [0x1021, 0xffff, false, false, 0x0000],
  'crc16-xmodem': [0x1021, 0x0000, false, false, 0x0000],
  'crc16-kermit': [0x1021, 0x0000, true, true, 0x0000],
}

export function computeCrc(algo: BytetideParser.CrcAlgo, bytes: Uint8Array): number {
  switch (algo) {
    case 'sum8':
      return crcSum8(bytes)
    case 'xor8':
      return crcXor8(bytes)
    case 'crc16-modbus':
    case 'crc16-ccitt-false':
    case 'crc16-xmodem':
    case 'crc16-kermit': {
      const [poly, init, refin, refout, xorout] = CRC16_PARAMS[algo]
      return crc16Generic(bytes, poly, init, refin, refout, xorout)
    }
    case 'crc32':
      return crc32(bytes)
  }
}
