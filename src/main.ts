import { createApp } from 'vue'
import { createPinia } from 'pinia'
import App from './App.vue'
import './styles.css'

// 屏蔽 WebView 默认菜单，保留组件自定义右键事件。
document.addEventListener('contextmenu', (event) => event.preventDefault(), { capture: true })

createApp(App).use(createPinia()).mount('#app')
