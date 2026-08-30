import { createApp } from 'vue'
import { createPinia } from 'pinia'
import 'vue-virtual-scroller/dist/vue-virtual-scroller.css'
import App from './App.vue'
import './styles.css'

createApp(App).use(createPinia()).mount('#app')
