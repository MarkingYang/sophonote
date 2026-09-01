/**
 * AG-25：FNV-1a 64 位哈希（TS 侧）。
 *
 * 与 Rust 侧 documents/repository.rs `content_hash` 完全同口径：
 * offset basis 0xcbf29ce484222325、prime 0x100000001b3、按 UTF-8 字节逐字节
 * XOR→乘、模 2^64、输出 16 位小写 hex。SelectionSnapshot.selectedTextHash /
 * beforeHash / afterHash 用它生成，Rust 侧 TextAnchor 解析用它校验——
 * 两侧算法必须逐位一致（空串向量 cbf29ce484222325 = offset basis，双侧共测）。
 */
const FNV_OFFSET = 0xcbf29ce484222325n;
const FNV_PRIME = 0x00000100000001b3n;
const MASK_64 = 0xffffffffffffffffn;

export function fnv1aHex(text: string): string {
  const bytes = new TextEncoder().encode(text);
  let hash = FNV_OFFSET;
  for (const byte of bytes) {
    hash ^= BigInt(byte);
    hash = (hash * FNV_PRIME) & MASK_64;
  }
  return hash.toString(16).padStart(16, '0');
}
