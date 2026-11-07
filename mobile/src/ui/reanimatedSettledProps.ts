type Collector = {
  viewsCount: number;
  viewsMap: Map<number, unknown>;
  unregisterInterval(): void;
  unregisterView(viewTag: number): void;
};

/**
 * Reanimated 4.5.1 counts a view once when a tag repeats, but uncounts it every
 * time. Unmounting many views at once, as a layout switch does, takes the count
 * to zero with views still alive, which stops the timer that copies settled
 * styles back into React. React then keeps an old value, and its next commit
 * puts it back on screen: the scene returned from Monos only still moved aside
 * for a drawer shut long before.
 *
 * Fixed on Reanimated's main branch, which goes by the map alone. Delete this
 * and applyReanimatedSettledProps.ts once the app is on a release with that fix.
 */
export function countOnlyRegisteredViews(collector: Collector) {
  collector.unregisterView = (viewTag) => {
    if (!collector.viewsMap.delete(viewTag)) return;
    collector.viewsCount--;
    if (collector.viewsCount === 0) collector.unregisterInterval();
  };
}
