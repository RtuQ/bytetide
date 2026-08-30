/// <reference types="vite/client" />

declare module "*.vue" {
  import type { DefineComponent } from "vue";
  const component: DefineComponent<{}, {}, any>;
  export default component;
}

// vue-virtual-scroller 未随包提供类型声明，按 any 处理
declare module "vue-virtual-scroller";
