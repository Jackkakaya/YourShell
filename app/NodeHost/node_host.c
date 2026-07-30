//
// node_host.c — starts the resident Node.js (nodejs-mobile) instance.
//
// node_start() runs node::InitializeOncePerProcess, which can only happen once
// per process. So we call it exactly once, on a dedicated thread, pointed at
// our resident dispatcher (main.js). That script never exits: it listens on a
// loopback port and runs each command the Rust core sends it, inside this one
// instance. See NodeResources/node/main.js.
//
// ys_node_start_resident(main_js_path) is called by the first node/npm/npx
// command. It loads NodeMobile.framework, spawns the node thread, and returns
// immediately. Later commands talk to the instance over TCP.
//

#include <dlfcn.h>
#include <limits.h>
#include <mach-o/dyld.h>
#include <pthread.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

static char *ys_main_js_path = NULL;
typedef int (*ys_node_start_fn)(int argc, char *argv[]);
static ys_node_start_fn ys_node_start = NULL;
static void *ys_node_runtime = NULL;
static pthread_mutex_t ys_node_mutex = PTHREAD_MUTEX_INITIALIZER;

static int ys_node_load_runtime(void) {
    if (ys_node_runtime != NULL) {
        return 0;
    }
    char executable[PATH_MAX];
    uint32_t size = sizeof(executable);
    if (_NSGetExecutablePath(executable, &size) != 0) {
        fprintf(stderr, "node: executable path exceeds PATH_MAX\n");
        return -1;
    }
    char *slash = strrchr(executable, '/');
    if (slash == NULL) {
        fprintf(stderr, "node: invalid executable path\n");
        return -1;
    }
    *slash = '\0';

    char path[PATH_MAX];
    int length = snprintf(path, sizeof(path),
                          "%s/Frameworks/NodeMobile.framework/NodeMobile",
                          executable);
    if (length < 0 || (size_t)length >= sizeof(path)) {
        fprintf(stderr, "node: framework path exceeds PATH_MAX\n");
        return -1;
    }
    void *handle = dlopen(path, RTLD_NOW | RTLD_LOCAL);
    if (handle == NULL) {
        fprintf(stderr, "node: cannot load %s: %s\n", path, dlerror());
        return -1;
    }
    *(void **)(&ys_node_start) = dlsym(handle, "node_start");
    if (ys_node_start == NULL) {
        fprintf(stderr, "node: NodeMobile.framework lacks node_start: %s\n",
                dlerror());
        dlclose(handle);
        return -1;
    }
    ys_node_runtime = handle;
    return 0;
}

static void *ys_node_thread(void *arg) {
    (void)arg;
    // V8 needs --jitless on iOS (no JIT entitlement) and this also avoids the
    // WebAssembly trap-handler registration that aborts on the simulator.
    char arg0[] = "node";
    char arg1[] = "--jitless";
    char *argv[] = {arg0, arg1, ys_main_js_path, NULL};
    ys_node_start(3, argv);
    return NULL;
}

int ys_node_start_resident(const char *main_js_path) {
    pthread_mutex_lock(&ys_node_mutex);
    if (ys_main_js_path != NULL) {
        pthread_mutex_unlock(&ys_node_mutex);
        return 0; // already started
    }
    if (ys_node_load_runtime() != 0) {
        pthread_mutex_unlock(&ys_node_mutex);
        return 125;
    }
    ys_main_js_path = strdup(main_js_path);
    if (ys_main_js_path == NULL) {
        pthread_mutex_unlock(&ys_node_mutex);
        fprintf(stderr, "node: cannot allocate dispatcher path\n");
        return 125;
    }

    pthread_attr_t attr;
    pthread_attr_init(&attr);
    // V8 wants a large stack.
    pthread_attr_setstacksize(&attr, 16 * 1024 * 1024);
    pthread_t thread;
    int create_result = pthread_create(&thread, &attr, ys_node_thread, NULL);
    pthread_attr_destroy(&attr);
    if (create_result != 0) {
        free(ys_main_js_path);
        ys_main_js_path = NULL;
        pthread_mutex_unlock(&ys_node_mutex);
        fprintf(stderr, "node: cannot create runtime thread: %s\n",
                strerror(create_result));
        return 125;
    }
    pthread_detach(thread);
    pthread_mutex_unlock(&ys_node_mutex);
    return 0;
}
