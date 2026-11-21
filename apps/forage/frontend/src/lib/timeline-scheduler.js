// One canonical request at a time, at most once per interval while visible.
export function createTimelineScheduler(run, { visible = true, interval = 10000, onError = () => {} } = {}) {
  let dirty = false;
  let stopped = false;
  let lastStarted = -Infinity;
  let timer = null;
  let pending = null;

  function cancelTimer() {
    clearTimeout(timer);
    timer = null;
  }

  function schedule() {
    cancelTimer();
    if (stopped || !visible || !dirty || pending) return;
    const delay = Math.max(0, lastStarted + interval - Date.now());
    if (delay > 0) {
      timer = setTimeout(schedule, delay);
      return;
    }
    dirty = false;
    lastStarted = Date.now();
    const controller = new AbortController();
    pending = controller;
    Promise.resolve().then(() => {
      if (!controller.signal.aborted) return run(controller.signal);
    }).catch(error => {
      if (!controller.signal.aborted) onError(error);
    }).finally(() => {
      if (pending === controller) pending = null;
      schedule();
    });
  }

  return {
    request() {
      if (stopped) return;
      dirty = true;
      schedule();
    },
    setVisible(value) {
      visible = value;
      if (!visible && pending) {
        dirty = true;
        pending.abort();
      }
      schedule();
    },
    destroy() {
      stopped = true;
      cancelTimer();
      pending?.abort();
    },
  };
}
