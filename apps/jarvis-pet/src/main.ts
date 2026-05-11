import { createApp } from 'vue'
import { createPinia } from 'pinia'
import App from './App.vue'
import { bridgeConsoleToFile } from './lib/petLog'

// 把所有 console.{log,warn,error} mirror 到 /tmp/jarvis-pet.log 便于外部 tail 看
bridgeConsoleToFile()

createApp(App).use(createPinia()).mount('#app')
