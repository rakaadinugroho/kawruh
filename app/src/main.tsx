import { createRoot } from 'react-dom/client'
import App from './App'
import { applyAppearance, cachedAppearance } from './lib/appearance'
import './styles.css'

applyAppearance(cachedAppearance())

createRoot(document.getElementById('root')!).render(<App />)
