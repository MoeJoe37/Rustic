package dev.rustic.android;

/** The broker side, in the app's main process (ADR 0059). */
public final class Broker {
    static {
        System.loadLibrary("rustic_android");
    }

    private Broker() {
    }

    /**
     * Run {@code dump-dom} on {@code files/page.html} against the renderer
     * at the other end of {@code rendererFd} (detached), write the output
     * to {@code outFd} (detached) and return the exit status.
     */
    public static native int nativeDumpDom(int rendererFd, int outFd);
}
