#import <AppKit/AppKit.h>
#import <UniformTypeIdentifiers/UniformTypeIdentifiers.h>
#include <stdlib.h>
#include <string.h>

extern void archive_app_receive_service_paths(const char *action, const char *paths_json);

@interface ArchiveAppServiceProvider : NSObject
- (void)archiveExtractHere:(NSPasteboard *)pasteboard userData:(NSString *)userData error:(NSString **)error;
- (void)archiveExtractToFolder:(NSPasteboard *)pasteboard userData:(NSString *)userData error:(NSString **)error;
- (void)archiveTest:(NSPasteboard *)pasteboard userData:(NSString *)userData error:(NSString **)error;
- (void)archiveCompressZip:(NSPasteboard *)pasteboard userData:(NSString *)userData error:(NSString **)error;
- (void)archiveCompressOptions:(NSPasteboard *)pasteboard userData:(NSString *)userData error:(NSString **)error;
@end

@implementation ArchiveAppServiceProvider

- (void)submit:(NSPasteboard *)pasteboard action:(NSString *)action error:(NSString **)error {
  NSArray<NSURL *> *urls = [pasteboard
      readObjectsForClasses:@[ [NSURL class] ]
                    options:@{NSPasteboardURLReadingFileURLsOnlyKey : @YES}];

  if (urls.count == 0) {
    if (error != NULL) {
      *error = @"Archive App did not receive any file URLs.";
    }
    return;
  }

  NSMutableArray<NSString *> *paths = [NSMutableArray arrayWithCapacity:urls.count];
  for (NSURL *url in urls) {
    if (url.isFileURL && url.path != nil) {
      [paths addObject:url.path];
    }
  }

  if (paths.count == 0) {
    if (error != NULL) {
      *error = @"Archive App did not receive any valid file URLs.";
    }
    return;
  }

  NSData *jsonData = [NSJSONSerialization dataWithJSONObject:paths options:0 error:nil];
  NSString *json = [[NSString alloc] initWithData:jsonData encoding:NSUTF8StringEncoding];
  archive_app_receive_service_paths(action.UTF8String, json.UTF8String);
}

- (void)archiveExtractHere:(NSPasteboard *)pasteboard userData:(NSString *)userData error:(NSString **)error {
  [self submit:pasteboard action:userData error:error];
}

- (void)archiveExtractToFolder:(NSPasteboard *)pasteboard userData:(NSString *)userData error:(NSString **)error {
  [self submit:pasteboard action:userData error:error];
}

- (void)archiveTest:(NSPasteboard *)pasteboard userData:(NSString *)userData error:(NSString **)error {
  [self submit:pasteboard action:userData error:error];
}

- (void)archiveCompressZip:(NSPasteboard *)pasteboard userData:(NSString *)userData error:(NSString **)error {
  [self submit:pasteboard action:userData error:error];
}

- (void)archiveCompressOptions:(NSPasteboard *)pasteboard userData:(NSString *)userData error:(NSString **)error {
  [self submit:pasteboard action:userData error:error];
}

@end

void archive_app_register_services(void) {
  static ArchiveAppServiceProvider *provider;
  provider = [[ArchiveAppServiceProvider alloc] init];
  NSApp.servicesProvider = provider;
  NSUpdateDynamicServices();
}

char *archive_app_icon_data_url(const char *key) {
  NSString *value = [NSString stringWithUTF8String:key];
  UTType *type = [value isEqualToString:@"__folder__"]
      ? UTTypeFolder
      : [UTType typeWithFilenameExtension:value];
  NSImage *icon = [[NSWorkspace sharedWorkspace]
      iconForContentType:(type != nil ? type : UTTypeData)];
  NSRect rect = NSMakeRect(0, 0, 32, 32);
  CGImageRef image = [icon CGImageForProposedRect:&rect context:nil hints:nil];
  if (image == nil) {
    return NULL;
  }
  NSBitmapImageRep *bitmap = [[NSBitmapImageRep alloc] initWithCGImage:image];
  NSData *png = [bitmap representationUsingType:NSBitmapImageFileTypePNG properties:@{}];
  NSString *base64 = [png base64EncodedStringWithOptions:0];
  NSString *url = [@"data:image/png;base64," stringByAppendingString:base64];
  return strdup(url.UTF8String);
}

void archive_app_free_string(char *value) {
  free(value);
}
