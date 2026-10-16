import adapter from '@sveltejs/adapter-static';
import { sveltekit } from '@sveltejs/kit/vite';
import { defineConfig } from 'vite';

export default defineConfig({
	plugins: [
		sveltekit({
			// Tauri serves static files; there is no server, so the app is a single-page app.
			adapter: adapter({ fallback: 'index.html' })
		})
	],
	// Tauri expects a fixed port and must see Rust errors in the terminal.
	clearScreen: false,
	server: {
		port: 1420,
		strictPort: true,
		host: '127.0.0.1',
		watch: { ignored: ['**/src-tauri/**'] }
	}
});
