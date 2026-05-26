#include "cua_driver_embedded.h"

#include <CoreFoundation/CoreFoundation.h>
#include <pthread.h>
#include <stdatomic.h>
#include <stdio.h>
#include <string.h>
#include <unistd.h>

typedef struct AppCheckContext {
    FILE *out;
    atomic_int done;
    int status;
} AppCheckContext;

static int require_contains(FILE *out, const char *label, const char *value, const char *needle) {
    if (value == NULL || strstr(value, needle) == NULL) {
        fprintf(out, "%s missing %s\n", label, needle);
        if (value != NULL) {
            fprintf(out, "%s response: %.600s\n", label, value);
        }
        fflush(out);
        return 1;
    }
    return 0;
}

static void finish(AppCheckContext *ctx, int status) {
    ctx->status = status;
    atomic_store(&ctx->done, 1);
    CFRunLoopStop(CFRunLoopGetMain());
}

static void *run_check(void *raw) {
    AppCheckContext *ctx = (AppCheckContext *)raw;
    FILE *out = ctx->out;

    fprintf(out, "pid=%d\n", getpid());
    fprintf(out, "worker=start\n");
    fflush(out);

    CuaDriver *driver = cua_driver_embedded_new(false);
    if (driver == NULL) {
        fprintf(out, "driver=create_failed\n");
        finish(ctx, 1);
        return NULL;
    }
    fprintf(out, "driver=created\n");
    fflush(out);

    char *initialize = cua_driver_embedded_handle_mcp_json(
        driver,
        "{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"initialize\"}"
    );
    fprintf(out, "initialize=returned\n");
    fflush(out);
    if (require_contains(out, "initialize", initialize, "\"name\":\"cua-driver\"")) {
        finish(ctx, 1);
        return NULL;
    }
    cua_driver_embedded_string_free(initialize);

    char *tools = cua_driver_embedded_handle_mcp_json(
        driver,
        "{\"jsonrpc\":\"2.0\",\"id\":2,\"method\":\"tools/list\"}"
    );
    fprintf(out, "tools_list=returned\n");
    fflush(out);
    if (require_contains(out, "tools/list", tools, "\"get_window_state\"") ||
        require_contains(out, "tools/list", tools, "\"check_permissions\"")) {
        finish(ctx, 1);
        return NULL;
    }
    fprintf(out, "tools_list_bytes=%zu\n", strlen(tools));
    cua_driver_embedded_string_free(tools);

    char *permissions = cua_driver_embedded_handle_mcp_json(
        driver,
        "{\"jsonrpc\":\"2.0\",\"id\":3,\"method\":\"tools/call\",\"params\":{\"name\":\"check_permissions\",\"arguments\":{\"prompt\":false}}}"
    );
    fprintf(out, "check_permissions=returned\n");
    fflush(out);
    if (require_contains(out, "check_permissions", permissions, "\"accessibility\"") ||
        require_contains(out, "check_permissions", permissions, "\"screen_recording\"")) {
        finish(ctx, 1);
        return NULL;
    }
    fprintf(out, "check_permissions_response=%s\n", permissions);
    cua_driver_embedded_string_free(permissions);

    char *notification = cua_driver_embedded_handle_mcp_json(
        driver,
        "{\"jsonrpc\":\"2.0\",\"method\":\"notifications/initialized\"}"
    );
    if (notification != NULL) {
        fprintf(out, "notification=unexpected_response\n");
        cua_driver_embedded_string_free(notification);
        finish(ctx, 1);
        return NULL;
    }
    fprintf(out, "notification=null\n");

    cua_driver_embedded_free(driver);
    fprintf(out, "result=passed\n");
    fflush(out);

    finish(ctx, 0);
    return NULL;
}

int main(int argc, char **argv) {
    const char *out_path = argc > 1 ? argv[1] : "/tmp/cua_embedded_app_check.out";
    FILE *out = fopen(out_path, "w");
    if (out == NULL) {
        return 1;
    }

    AppCheckContext ctx = {
        .out = out,
        .done = 0,
        .status = 1,
    };

    pthread_t thread;
    if (pthread_create(&thread, NULL, run_check, &ctx) != 0) {
        fprintf(out, "worker=create_failed\n");
        fclose(out);
        return 1;
    }

    while (!atomic_load(&ctx.done)) {
        CFRunLoopRunInMode(kCFRunLoopDefaultMode, 0.05, false);
    }

    pthread_join(thread, NULL);
    fclose(out);
    return ctx.status;
}
