export function shouldDeferResidentUi(startedHidden: boolean, backendWindowVisible: boolean) {
  return startedHidden && !backendWindowVisible;
}
