package dev.rustic.android;

import android.app.Service;
import android.content.Intent;
import android.os.Binder;
import android.os.IBinder;
import android.os.Parcel;
import android.os.ParcelFileDescriptor;
import android.os.RemoteException;

/**
 * A Rustic renderer in an isolated process (ADR 0059). The broker sends it
 * one end of a socket pair; the native renderer serves the broker over it
 * with the same IPC as on desktop. The service opens nothing itself.
 */
public class RendererService extends Service {
    static {
        System.loadLibrary("rustic_android");
    }

    /** Serve the broker over {@code fd}; with {@code crash}, abort at once. */
    static native int nativeRun(int fd, boolean crash);

    /** Try a compromised renderer's escapes; returns the bits that worked. */
    static native int nativeProbe(int port);

    /** Transaction: run a renderer over a socket (ParcelFileDescriptor, int crash). */
    public static final int RUN = IBinder.FIRST_CALL_TRANSACTION;
    /** Transaction: run the probes (int port) and reply with their bits. */
    public static final int PROBE = IBinder.FIRST_CALL_TRANSACTION + 1;
    /** Transaction: reply whether this process is isolated. */
    public static final int IS_ISOLATED = IBinder.FIRST_CALL_TRANSACTION + 2;

    private final Binder binder = new Binder() {
        @Override
        protected boolean onTransact(int code, Parcel data, Parcel reply, int flags)
                throws RemoteException {
            switch (code) {
                case RUN: {
                    ParcelFileDescriptor socket = data.readParcelable(
                            ParcelFileDescriptor.class.getClassLoader());
                    boolean crash = data.readInt() != 0;
                    if (socket == null) {
                        return false;
                    }
                    int fd = socket.detachFd();
                    new Thread(() -> nativeRun(fd, crash), "rustic renderer").start();
                    if (reply != null) {
                        reply.writeNoException();
                    }
                    return true;
                }
                case PROBE: {
                    int bits = nativeProbe(data.readInt());
                    reply.writeNoException();
                    reply.writeInt(bits);
                    return true;
                }
                case IS_ISOLATED: {
                    reply.writeNoException();
                    reply.writeInt(android.os.Process.isIsolated() ? 1 : 0);
                    return true;
                }
                default:
                    return super.onTransact(code, data, reply, flags);
            }
        }
    };

    @Override
    public IBinder onBind(Intent intent) {
        return binder;
    }
}
