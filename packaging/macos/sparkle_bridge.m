#import <Foundation/Foundation.h>
#import <Sparkle/Sparkle.h>

static SPUStandardUpdaterController *gController;

void myproxy_sparkle_init(void) {
    @autoreleasepool {
        NSString *path = [[NSBundle mainBundle] bundlePath];
        if (![path hasSuffix:@".app"]) {
            return;
        }
        if (gController != nil) {
            return;
        }
        gController = [[SPUStandardUpdaterController alloc]
            initWithStartingUpdater:YES
                    updaterDelegate:nil
                 userDriverDelegate:nil];
    }
}

void myproxy_sparkle_check(void) {
    @autoreleasepool {
        myproxy_sparkle_init();
        if (gController == nil) {
            return;
        }
        [gController checkForUpdates:nil];
    }
}
