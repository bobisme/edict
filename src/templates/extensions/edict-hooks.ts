import type { ExtensionAPI } from "@mariozechner/pi-coding-agent";

async function runHook(
	pi: ExtensionAPI,
	hookName: string,
	extraArgs: string[] = [],
): Promise<string | null> {
	try {
		const result = await pi.exec("edict", ["hooks", "run", hookName, ...extraArgs]);
		if (result.code !== 0) {
			return null;
		}

		const stdout = result.stdout?.trim();
		return stdout && stdout.length > 0 ? stdout : null;
	} catch {
		// Graceful degradation when botbox is not installed or hook execution fails.
		return null;
	}
}

function injectMessage(pi: ExtensionAPI, stdout: string) {
	pi.sendMessage({
		customType: "edict-hook",
		content: stdout,
		display: false,
	});
}

export default function edictHooksExtension(pi: ExtensionAPI) {
	let toolResultCount = 0;
	let pendingSessionStartContext = "";

	pi.on("session_start", async () => {
		const stdout = await runHook(pi, "session-start");
		if (stdout) {
			pendingSessionStartContext = stdout;
		}
	});

	pi.on("before_agent_start", async (event) => {
		if (!pendingSessionStartContext) {
			return;
		}

		const injected = pendingSessionStartContext;
		pendingSessionStartContext = "";

		return {
			systemPrompt: `${event.systemPrompt}\n\n${injected}`,
		};
	});

	pi.on("tool_result", async () => {
		toolResultCount += 1;
		if (toolResultCount % 5 !== 0) {
			return;
		}

		const stdout = await runHook(pi, "post-tool-call");
		if (stdout) {
			injectMessage(pi, stdout);
		}
	});

	pi.on("session_before_compact", async () => {
		const stdout = await runHook(pi, "session-start");
		if (stdout) {
			injectMessage(pi, stdout);
		}
	});

	pi.on("session_shutdown", async () => {
		await runHook(pi, "session-end");
	});
}
