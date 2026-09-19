// vitest 全局 setup：测试环境锁定中文界面语言。
// jsdom 的 navigator.language 是 en-US，而生产默认语言跟随系统（i18n
// detectSystemLocale）——不锁定会让既有中文断言在测试里漂移成英文。
import { setLocale } from '../i18n'

setLocale('zh-CN')
