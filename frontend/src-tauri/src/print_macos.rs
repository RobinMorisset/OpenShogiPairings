//! Printing on macOS: build the WKWebView's print operation ourselves, and
//! answer only once AppKit says it has run.
//!
//! Two of the app's print buttons print something *other* than what is on
//! screen — the QR code for the wall, and the result sheets. Both work by
//! putting their document into the DOM, printing, and taking it out again. That
//! makes *when the print is over* part of the contract: tear the document down
//! too early and the referee gets whatever the app happened to look like
//! instead — for the sheets button, a printout of the round draft view.
//!
//! That is exactly what wry's `WebView::print` gives us: it starts an AppKit
//! *modal* print operation (`runOperationModalForWindow:…`) and returns
//! immediately, long before the page is rendered. So instead of calling it, we
//! build the same `NSPrintOperation` here and pass it a `didRunSelector`
//! target, which is AppKit's "the operation has finished" callback.

use std::ffi::c_void;

use objc2::rc::Retained;
use objc2::runtime::{AnyObject, Bool, NSObject, NSObjectProtocol};
use objc2::{define_class, msg_send, sel, AllocAnyThread};
use objc2_app_kit::{NSPaperOrientation, NSPrintInfo, NSWindow};
use objc2_web_kit::WKWebView;
use tauri::async_runtime::{channel, Sender};
use tauri::webview::PlatformWebview;

/// One reply per print job: `Ok(())` once AppKit reports the operation has run
/// (whether the referee printed or cancelled it), or why it never started.
type Reply = Result<(), String>;

define_class!(
    /// The `didRunSelector` target below. It holds no state of its own — the
    /// channel to answer on travels in `contextInfo` — so one instance serves
    /// every print job.
    ///
    /// SAFETY:
    /// - `NSObject` has no subclassing requirements.
    /// - This type does not implement `Drop`.
    #[unsafe(super(NSObject))]
    #[name = "OspPrintCompletionDelegate"]
    struct PrintCompletion;

    impl PrintCompletion {
        /// AppKit's completion callback, shaped exactly as
        /// `runOperationModalForWindow:delegate:didRunSelector:contextInfo:`
        /// documents it.
        #[unsafe(method(printOperationDidRun:success:contextInfo:))]
        fn print_operation_did_run(
            &self,
            _operation: *mut AnyObject,
            success: Bool,
            context_info: *mut c_void,
        ) {
            // `context_info` is the box [`start`] leaked, and AppKit calls this
            // exactly once per operation, so taking ownership back here is both
            // what answers the waiting command and what frees it.
            let reply: Box<Sender<Reply>> = unsafe { Box::from_raw(context_info.cast()) };
            if !success.as_bool() {
                // Cancelling the print panel lands here too — by far the common
                // case, and not an error — and AppKit gives us nothing to tell
                // it apart from a print that genuinely failed. Hence a log line
                // rather than an error banner: the print is over either way.
                log::info!("print operation reported no success (cancelled, or failed)");
            }
            let _ = reply.try_send(Ok(()));
        }
    }
);

impl PrintCompletion {
    fn new() -> Retained<Self> {
        let this = Self::alloc().set_ivars(());
        unsafe { msg_send![super(this), init] }
    }
}

thread_local! {
    /// Created once and kept for the life of the (main) thread: AppKit does not
    /// retain a `didRunSelector` target, and the callback arrives whenever the
    /// referee is done with the print panel.
    static COMPLETION: Retained<PrintCompletion> = PrintCompletion::new();
}

/// Print `window`, resolving when the print operation has finished.
///
/// `landscape` requests landscape paper; see [`start`] for why it cannot be
/// left to the CSS `@page` rules here.
pub(crate) async fn print_and_wait(window: tauri::WebviewWindow, landscape: bool) -> Reply {
    let (reply, mut replies) = channel::<Reply>(1);
    window
        .with_webview(move |webview| {
            // Runs on the event loop's thread, which is the main thread — where
            // every AppKit call below belongs.
            if let Err(err) = start(&webview, landscape, reply.clone()) {
                let _ = reply.try_send(Err(err));
            }
        })
        .map_err(|err| format!("could not reach the window to print it: {err}"))?;
    // `None` means the closure above was dropped without running (the window
    // went away); a print that started always answers, success or not.
    replies
        .recv()
        .await
        .unwrap_or_else(|| Err("the print job never reported back".into()))
}

/// Start the print operation, handing it the channel to answer on.
fn start(platform: &PlatformWebview, landscape: bool, reply: Sender<Reply>) -> Reply {
    // SAFETY: on macOS these are the window's WKWebView and its NSWindow, both
    // alive for as long as the window this command was invoked from.
    let webview: &WKWebView = unsafe { &*platform.inner().cast() };
    let window: &NSWindow = unsafe { &*platform.ns_window().cast() };

    if !webview.respondsToSelector(sel!(printOperationWithPrintInfo:)) {
        return Err("this webview cannot print (macOS 11 or newer is needed)".into());
    }

    // The *shared* print info, as wry's own print built it — and a global, so
    // every job sets everything it depends on rather than inheriting the last
    // one's choices.
    let info = NSPrintInfo::sharedPrintInfo();
    // wry never touched orientation, and WebKit's print path ignores the CSS
    // `@page { size: landscape }` the browser build relies on, so the wide
    // Results table would print portrait unless we set it here.
    info.setOrientation(if landscape {
        NSPaperOrientation::Landscape
    } else {
        NSPaperOrientation::Portrait
    });
    // Zero margins, again as wry's print set them: the print layouts (the slip
    // geometry in `resultSheets.ts`, the pairing and results tables) were
    // measured against that, and inheriting whatever the shared info carried
    // would silently change how much of the page they get.
    info.setTopMargin(0.0);
    info.setRightMargin(0.0);
    info.setBottomMargin(0.0);
    info.setLeftMargin(0.0);

    // SAFETY: the selector was just checked for, and the print info is a valid
    // NSPrintInfo.
    let operation = unsafe { webview.printOperationWithPrintInfo(&info) };
    // Let AppKit render off the main thread, as wry did: a long tournament's
    // sheets are a good many pages.
    operation.setCanSpawnSeparateThread(true);
    let context_info = Box::into_raw(Box::new(reply)).cast::<c_void>();
    COMPLETION.with(|delegate| {
        let delegate: &AnyObject = AsRef::as_ref(&**delegate);
        // SAFETY: `delegate` implements the selector named right after it, and
        // `context_info` is the box it takes ownership of.
        unsafe {
            operation.runOperationModalForWindow_delegate_didRunSelector_contextInfo(
                window,
                Some(delegate),
                Some(sel!(printOperationDidRun:success:contextInfo:)),
                context_info,
            );
        }
    });
    Ok(())
}
