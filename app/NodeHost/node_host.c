//
// node_host.c — starts the resident Node.js (nodejs-mobile) instance.
//
// node_start() runs node::InitializeOncePerProcess, which can only happen once
// per process. So we call it exactly once, on a dedicated thread, pointed at
// our resident dispatcher (main.js). That script never exits: it listens on a
// loopback port and runs each command the Rust core sends it, inside this one
// instance. See NodeResources/node/main.js.
//
// ys_node_start_resident(main_js_path) is called once at app launch; it
// spawns the node thread and returns immediately. The Rust `node` builtin
// then talks to the instance over TCP — it never calls node_start itself.
//

#include <NodeMobile/NodeMobile.h>
#include <pthread.h>
#include <stdlib.h>
#include <string.h>

static char *ys_main_js_path = NULL;

static void *ys_node_thread(void *arg) {
    (void)arg;
    // V8 needs --jitless on iOS (no JIT entitlement) and this also avoids the
    // WebAssembly trap-handler registration that aborts on the simulator.
    char arg0[] = "node";
    char arg1[] = "--jitless";
    char *argv[] = {arg0, arg1, ys_main_js_path, NULL};
    node_start(3, argv);
    return NULL;
}

void ys_node_start_resident(const char *main_js_path) {
    if (ys_main_js_path != NULL) {
        return; // already started
    }
    ys_main_js_path = strdup(main_js_path);

    pthread_attr_t attr;
    pthread_attr_init(&attr);
    // V8 wants a large stack.
    pthread_attr_setstacksize(&attr, 16 * 1024 * 1024);
    pthread_t thread;
    pthread_create(&thread, &attr, ys_node_thread, NULL);
    pthread_attr_destroy(&attr);
    pthread_detach(thread);
}
