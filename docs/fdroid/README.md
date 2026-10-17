# F-Droid

`app.termchat.yml` is a draft of termchat's entry in
[fdroiddata](https://gitlab.com/fdroid/fdroiddata). F-Droid builds the app
from source and, since the build is reproducible, ships the APK from our
GitHub release when its own build matches it byte for byte. Both sources then
carry the same signature and can update each other.

To submit: fork fdroiddata on GitLab, add the file as
`metadata/app.termchat.yml`, check it with `fdroid lint app.termchat` and
`fdroid build -v -l app.termchat`, then open a merge request. The store texts,
icon and screenshots are read from `android/fastlane/metadata/android/en-US`.
