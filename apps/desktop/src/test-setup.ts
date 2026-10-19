// jsdom has no layout or canvas, and its stubs log "Not implemented" on every call. The router
// scrolls to the top on each navigation and axe probes a canvas, so no-ops keep test output
// readable. `getContext` returns `null` as jsdom's own stub does, minus the log.
window.scrollTo = () => undefined;
HTMLCanvasElement.prototype.getContext = () => null;
