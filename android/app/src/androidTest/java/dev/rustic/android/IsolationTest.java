package dev.rustic.android;

import static org.junit.Assert.assertEquals;
import static org.junit.Assert.assertNotEquals;
import static org.junit.Assert.assertTrue;

import android.content.ComponentName;
import android.content.Context;
import android.content.Intent;
import android.content.ServiceConnection;
import android.os.IBinder;
import android.os.Parcel;
import android.os.ParcelFileDescriptor;

import androidx.test.ext.junit.runners.AndroidJUnit4;
import androidx.test.platform.app.InstrumentationRegistry;

import org.junit.Test;
import org.junit.runner.RunWith;

import java.io.ByteArrayOutputStream;
import java.io.File;
import java.io.FileOutputStream;
import java.io.InputStream;
import java.net.ServerSocket;
import java.nio.charset.StandardCharsets;
import java.util.concurrent.CountDownLatch;
import java.util.concurrent.TimeUnit;
import java.util.concurrent.atomic.AtomicReference;

/**
 * ISO-08 on the Android emulator (ADR 0059): the renderer runs in an
 * isolated service and serves the broker over a socket; the isolated
 * process cannot read or list the app's files, open sockets or start a
 * shell, which the same code in an ordinary process can; a renderer that
 * crashes fails only its command, and a fresh one serves the next.
 */
@RunWith(AndroidJUnit4.class)
public class IsolationTest {
    private static final int READ_APP_FILE = 1;
    private static final int LIST_APP_FILES = 2;
    private static final int CONNECT_LOOPBACK = 4;
    private static final int SPAWN_SHELL = 8;

    private final Context context =
            InstrumentationRegistry.getInstrumentation().getTargetContext();

    /** A bound service and its connection. */
    private static final class Bound {
        final IBinder binder;
        final ServiceConnection connection;

        Bound(IBinder binder, ServiceConnection connection) {
            this.binder = binder;
            this.connection = connection;
        }
    }

    private Bound bind(Class<?> service) throws Exception {
        CountDownLatch connected = new CountDownLatch(1);
        AtomicReference<IBinder> binder = new AtomicReference<>();
        ServiceConnection connection = new ServiceConnection() {
            @Override
            public void onServiceConnected(ComponentName name, IBinder service) {
                binder.set(service);
                connected.countDown();
            }

            @Override
            public void onServiceDisconnected(ComponentName name) {
            }
        };
        assertTrue(context.bindService(
                new Intent(context, service), connection, Context.BIND_AUTO_CREATE));
        assertTrue("service did not connect", connected.await(30, TimeUnit.SECONDS));
        return new Bound(binder.get(), connection);
    }

    private static int call(IBinder binder, int code, int argument) throws Exception {
        Parcel data = Parcel.obtain();
        Parcel reply = Parcel.obtain();
        try {
            data.writeInt(argument);
            binder.transact(code, data, reply, 0);
            reply.readException();
            return reply.readInt();
        } finally {
            data.recycle();
            reply.recycle();
        }
    }

    private void writeFile(String name, String text) throws Exception {
        try (FileOutputStream out = new FileOutputStream(new File(context.getFilesDir(), name))) {
            out.write(text.getBytes(StandardCharsets.UTF_8));
        }
    }

    /** Run dump-dom against a renderer in {@code bound}; returns {status, output}. */
    private Object[] dumpDom(Bound bound, boolean crash) throws Exception {
        ParcelFileDescriptor[] socket = ParcelFileDescriptor.createSocketPair();
        Parcel data = Parcel.obtain();
        Parcel reply = Parcel.obtain();
        try {
            data.writeParcelable(socket[1], 0);
            data.writeInt(crash ? 1 : 0);
            bound.binder.transact(RendererService.RUN, data, reply, 0);
            reply.readException();
        } catch (android.os.DeadObjectException died) {
            // The crashing renderer may die before it replies.
        } finally {
            data.recycle();
            reply.recycle();
            socket[1].close();
        }
        ParcelFileDescriptor[] pipe = ParcelFileDescriptor.createPipe();
        AtomicReference<byte[]> output = new AtomicReference<>();
        Thread reader = new Thread(() -> {
            try (InputStream in = new ParcelFileDescriptor.AutoCloseInputStream(pipe[0])) {
                ByteArrayOutputStream bytes = new ByteArrayOutputStream();
                byte[] buffer = new byte[8192];
                int read;
                while ((read = in.read(buffer)) > 0) {
                    bytes.write(buffer, 0, read);
                }
                output.set(bytes.toByteArray());
            } catch (Exception error) {
                output.set(new byte[0]);
            }
        });
        reader.start();
        int status = Broker.nativeDumpDom(socket[0].detachFd(), pipe[1].detachFd());
        reader.join(30_000);
        return new Object[] {status, new String(output.get(), StandardCharsets.UTF_8)};
    }

    @Test
    public void theRendererServiceIsIsolatedAndCannotEscape() throws Exception {
        writeFile("secret.txt", "secret");
        try (ServerSocket listener = new ServerSocket(0, 50,
                java.net.InetAddress.getByName("127.0.0.1"))) {
            int port = listener.getLocalPort();
            Bound control = bind(ProbeService.class);
            Bound renderer = bind(RendererService.class);
            try {
                assertEquals(0, call(control.binder, RendererService.IS_ISOLATED, 0));
                assertEquals(1, call(renderer.binder, RendererService.IS_ISOLATED, 0));
                int open = call(control.binder, RendererService.PROBE, port);
                assertEquals("control process reaches everything",
                        READ_APP_FILE | LIST_APP_FILES | CONNECT_LOOPBACK | SPAWN_SHELL, open);
                int confined = call(renderer.binder, RendererService.PROBE, port);
                assertEquals("isolated renderer reaches nothing", 0, confined);
            } finally {
                context.unbindService(control.connection);
                context.unbindService(renderer.connection);
            }
        }
    }

    @Test
    public void theIsolatedRendererServesTheBrokerAndACrashIsRecovered() throws Exception {
        writeFile("page.html", "<!doctype html><p id=t>isolated</p>");
        Bound first = bind(RendererService.class);
        Object[] crashed;
        try {
            crashed = dumpDom(first, true);
        } finally {
            context.unbindService(first.connection);
        }
        assertNotEquals("a crashed renderer fails its command", 0, crashed[0]);
        assertEquals("", crashed[1]);

        Bound second = bind(RendererService.class);
        try {
            Object[] served = dumpDom(second, false);
            assertEquals(served[1].toString(), 0, served[0]);
            assertTrue(served[1].toString(), served[1].toString().contains("isolated"));
        } finally {
            context.unbindService(second.connection);
        }
    }
}
