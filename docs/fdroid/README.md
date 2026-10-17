# F-Droid

`app.termchat.yml` is a draft of termchat's entry in
[fdroiddata](https://gitlab.com/fdroid/fdroiddata). F-Droid builds the app
from source and signs it with its own key, so an APK from F-Droid and one from
GitHub releases cannot update each other: pick one source and stay with it.

To submit: fork fdroiddata on GitLab, add the file as
`metadata/app.termchat.yml`, check it with `fdroid lint app.termchat` and
`fdroid build -v -l app.termchat`, then open a merge request. The store texts,
icon and screenshots are read from `android/fastlane/metadata/android/en-US`.
