<script setup lang="ts" generic="T">
import { computed, onBeforeUnmount, onMounted, ref, watch } from 'vue'
import { computeWindow, reanchorStart } from '../composables/virtualWindow'

/**
 * 定高虚拟滚动（LogView 专用，替代 vue-virtual-scroller 的 RecycleScroller）。
 *
 * 与池复用型虚拟滚动的本质差异：DOM 按 key-field（行 no）稳定 key 渲染——
 * 追加/头部裁剪/回补只改既有节点的 top（style patch）或增删窗口边缘节点，
 * 绝不把已渲染节点改内容复用给别的行。浏览器文本选区锚定在节点上，
 * 因此「接收中拖选/复制」不再被打断（vue-virtual-scroller 每次 items 变化
 * 都整池释放重建，选区必丢，这是换掉它的原因，见 useRecycleScroller 源码
 * items watcher 的强制更新路径）。
 *
 * 窗口 = 可见区 ± buffer 像素缓冲；items 变化时以旧窗口起点行 reanchor
 * （键单调二分，失配回退长度差推算），保证同行节点跨批次存活。
 */
const props = withDefaults(
  defineProps<{
    items: T[]
    itemSize?: number
    keyField?: string
    /** 视口外上下缓冲（像素），保证滚动时边缘行已就绪 */
    buffer?: number
  }>(),
  { itemSize: 22, keyField: 'no', buffer: 200 },
)

const el = ref<HTMLElement | null>(null)
const start = ref(0)
const end = ref(0)

function keyOf(it: T): string | number {
  return (it as Record<string, unknown>)[props.keyField] as string | number
}

const totalH = computed(() => props.items.length * props.itemSize)
const rows = computed(() => props.items.slice(start.value, end.value))

function recompute() {
  const node = el.value
  if (!node) return
  const bufRows = Math.ceil(props.buffer / props.itemSize)
  const w = computeWindow(node.scrollTop, node.clientHeight, props.items.length, props.itemSize, bufRows)
  start.value = w.start
  end.value = w.end
}

// items 变化（拉取追加/回补/头部裁剪/整表替换）时重锚窗口起点：
// 先求旧起点行的新下标，保持窗口内 key 集合尽量不变，随后 scroll/补偿
// 逻辑（LogView 的 anchoredTop）再对齐 scrollTop。
watch(
  () => props.items,
  (n, o) => {
    if (!o || !el.value) {
      recompute()
      return
    }
    const count = Math.max(end.value - start.value, 1)
    const s = reanchorStart(o, n, start.value, keyOf)
    start.value = s
    end.value = Math.min(n.length, s + count)
  },
)

function scrollToItem(index: number) {
  const node = el.value
  if (!node) return
  const max = Math.max(0, totalH.value - node.clientHeight)
  node.scrollTop = Math.min(Math.max(index * props.itemSize, 0), max)
  recompute()
}

let ro: ResizeObserver | null = null
onMounted(() => {
  recompute()
  ro = new ResizeObserver(recompute)
  ro.observe(el.value!)
})
onBeforeUnmount(() => ro?.disconnect())

defineExpose({ el, scrollToItem })
</script>

<template>
  <div ref="el" class="ls-root" @scroll="recompute">
    <div class="ls-sizer" :style="{ height: `${totalH}px` }">
      <div
        v-for="(it, i) in rows"
        :key="keyOf(it)"
        class="ls-row"
        :style="{ top: `${(start + i) * itemSize}px` }"
      >
        <slot :item="it" :index="start + i" />
      </div>
    </div>
  </div>
</template>
