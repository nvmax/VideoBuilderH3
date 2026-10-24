// Stand-in for ComfyUI's `scripts/app.js`. The Builder's front end only asks it for the graph (to mark it changed), the
// canvas and `registerExtension`. There is no node graph in H3 Studio, so these do nothing, and extensions that register
// themselves (the resource monitor, the themes) are kept to be started by the boot script.

export const extensions = [];

export const app = {
  clientId: undefined,
  graph: {
    _nodes: [],
    setDirtyCanvas() {},
    add() {},
  },
  canvas: { graph_mouse: [0, 0], graph: null, setDirty() {}, draw() {} },
  extensionManager: { toast: { add() {} } },
  registerExtension(extension) {
    extensions.push(extension);
  },
  async queuePrompt() {
    throw new Error("The graph queue is not part of H3 Studio.");
  },
};

export default app;
