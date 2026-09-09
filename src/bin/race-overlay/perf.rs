// Rust guideline compliant 2026-02-16

//! Keeping the overlay's background work out of iRacing's way.
//!
//! iRacing is acutely sensitive to CPU starvation — a physics or graphics
//! thread that misses its slice can stutter or glitch the whole sim — so the
//! overlay's own threads must be good neighbours. The threads that do
//! continuous work (the telemetry poll that builds a snapshot every tick,
//! the input poll, and team sync's socket pumps) mark
//! themselves **`EcoQoS`**: Windows then schedules them on efficiency cores
//! where the hardware has them, and below foreground threads everywhere, so
//! the sim's own threads win every contended slice.
//!
//! The overlay's render thread is deliberately *not* marked — it must stay
//! responsive — but it already paces itself (see `app::OverlayApp::run`), so
//! it is idle most of every frame regardless.
//!
//! The hint is best-effort: a failure is ignored, because a thread that could
//! not be de-prioritised still does its job — it just competes normally.

use windows::Win32::System::Threading::{
    GetCurrentThread, SetThreadInformation, THREAD_POWER_THROTTLING_CURRENT_VERSION,
    THREAD_POWER_THROTTLING_EXECUTION_SPEED, THREAD_POWER_THROTTLING_STATE, ThreadPowerThrottling,
};

/// Marks the calling thread as background work (`EcoQoS`).
///
/// Call once at the top of a long-lived worker thread. The scheduler then
/// prefers efficiency cores for it and runs it below foreground threads, so
/// iRacing's own threads are never preempted by the overlay's on a contended
/// core.
///
/// `ControlMask` says which throttling policies this call sets;
/// `StateMask` says whether to turn each on — both name
/// `THREAD_POWER_THROTTLING_EXECUTION_SPEED`, so this enables it. Zeroing
/// `StateMask` instead would turn it back off, which nothing here needs.
pub fn mark_background_thread() {
    let state = THREAD_POWER_THROTTLING_STATE {
        Version: THREAD_POWER_THROTTLING_CURRENT_VERSION,
        ControlMask: THREAD_POWER_THROTTLING_EXECUTION_SPEED,
        StateMask: THREAD_POWER_THROTTLING_EXECUTION_SPEED,
    };
    // SAFETY: `GetCurrentThread` returns a pseudo-handle for the calling
    // thread with no lifetime to manage. `SetThreadInformation` reads `state`
    // only for the duration of the call — a stack value that outlives it — and
    // the size passed matches the struct. No memory is shared or retained past
    // the call, so a failure (older Windows, or the feature disabled) is safe
    // to ignore.
    unsafe {
        let _ = SetThreadInformation(
            GetCurrentThread(),
            ThreadPowerThrottling,
            std::ptr::from_ref(&state).cast(),
            u32::try_from(size_of::<THREAD_POWER_THROTTLING_STATE>()).unwrap_or(0),
        );
    }
}
