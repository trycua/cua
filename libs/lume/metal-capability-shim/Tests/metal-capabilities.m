#import <Foundation/Foundation.h>
#import <Metal/Metal.h>

#include <errno.h>
#include <stdlib.h>

static BOOL parseFamily(const char *raw, NSUInteger *family) {
    errno = 0;
    char *end = NULL;
    unsigned long long parsed = strtoull(raw, &end, 0);
    if (errno != 0 || end == raw || !end || *end != '\0') return NO;
    *family = (NSUInteger)parsed;
    return YES;
}

int main(int argc, const char *argv[]) {
    @autoreleasepool {
        NSUInteger family = 1009;
        if (argc > 1 && !parseFamily(argv[1], &family)) {
            fprintf(stderr, "invalid family: %s\n", argv[1]);
            return 2;
        }

        id<MTLDevice> device = MTLCreateSystemDefaultDevice();
        if (!device) {
            fprintf(stderr, "Metal device unavailable\n");
            return 1;
        }

        printf("device=%s\n", device.name.UTF8String);
        printf("family=%llu\n", (unsigned long long)family);
        printf(
            "supports_family=%s\n",
            [device supportsFamily:(MTLGPUFamily)family] ? "true" : "false"
        );
        printf(
            "max_threadgroup_memory=%llu\n",
            (unsigned long long)device.maxThreadgroupMemoryLength
        );
    }
    return 0;
}
