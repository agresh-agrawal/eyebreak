plugins {
    id("com.android.application")
    id("org.jetbrains.kotlin.android")
}

// Release signing comes from CI secrets (see .github/workflows/android.yml).
// Without them the release APK is signed with the debug key.
val keystorePath: String? = System.getenv("EB_KEYSTORE")

android {
    namespace = "io.github.agreshagrawal.eyebreak"
    compileSdk = 35

    defaultConfig {
        applicationId = "io.github.agreshagrawal.eyebreak"
        minSdk = 26
        targetSdk = 35
        versionCode = 1
        versionName = "3.1.0"
    }

    signingConfigs {
        create("release") {
            if (keystorePath != null) {
                storeFile = file(keystorePath)
                storePassword = System.getenv("EB_KEYSTORE_PASSWORD")
                keyAlias = "eyebreak"
                keyPassword = System.getenv("EB_KEYSTORE_PASSWORD")
                storeType = "pkcs12"
            }
        }
    }

    buildTypes {
        release {
            isMinifyEnabled = true
            isShrinkResources = true
            proguardFiles(getDefaultProguardFile("proguard-android-optimize.txt"), "proguard-rules.pro")
            signingConfig = signingConfigs.getByName(if (keystorePath != null) "release" else "debug")
        }
    }

    compileOptions {
        sourceCompatibility = JavaVersion.VERSION_17
        targetCompatibility = JavaVersion.VERSION_17
    }
    kotlinOptions {
        jvmTarget = "17"
    }
}
