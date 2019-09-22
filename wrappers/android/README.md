## Indy SDK for Android
### Build Instructions

#### Using docker (recommended)

- Run `build_using_docker.sh`. This will build the aar file and put it in folder `wrappers/android/aar`

#### Without docker
- Make sure you have android-sdk installed
- Run the `build.sh`
- Aar should be present in the `wrappers/android/aar` after the successful build.

#### Change the version of libindy
- In `build.sh` change the version number in variable `libindy_version`
- Note: only stable release of libindy is supported as of now by the script.


Sample app for the usage of this wrapper is present in folder `samples/android/WrapperTest`